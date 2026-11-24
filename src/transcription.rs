use std::{env, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use teloxide::{net::Download, prelude::*, types::ReplyParameters};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    time::timeout,
};
use tracing::warn;

const MAX_BYTES: u64 = 20_000_000;
const MAX_SECONDS: u32 = 600;
type Error = Box<dyn std::error::Error + Send + Sync>;

struct Job {
    dir: tempfile::TempDir,
    response: oneshot::Sender<Result<String, String>>,
    progress: watch::Sender<String>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub struct Transcriber {
    sender: mpsc::Sender<Job>,
    slots: Arc<Semaphore>,
}

#[derive(Serialize)]
struct Request {
    id: u64,
    path: PathBuf,
}
#[derive(Deserialize)]
struct Response {
    id: Option<u64>,
    text: Option<String>,
    partial: Option<String>,
    error: Option<String>,
    ready: Option<bool>,
}

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Worker {
    async fn start(python: &str, script: &str) -> Result<Self, Error> {
        let mut command = Command::new(python);
        command.arg(script);
        Self::spawn(&mut command).await
    }

    async fn spawn(command: &mut Command) -> Result<Self, Error> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().ok_or("worker stdin unavailable")?;
        let output = BufReader::new(child.stdout.take().ok_or("worker stdout unavailable")?);
        let mut worker = Self {
            child,
            input,
            output,
        };
        let ready = timeout(Duration::from_secs(120), worker.read()).await??;
        if ready.ready != Some(true) {
            return Err("worker did not start".into());
        }
        Ok(worker)
    }

    async fn read(&mut self) -> Result<Response, Error> {
        let mut line = String::new();
        if self.output.read_line(&mut line).await? == 0 {
            return Err("worker exited".into());
        }
        Ok(serde_json::from_str(&line)?)
    }

    async fn transcribe(
        &mut self,
        id: u64,
        path: PathBuf,
        progress: &watch::Sender<String>,
    ) -> Result<String, Error> {
        let mut request = serde_json::to_vec(&Request { id, path })?;
        request.push(b'\n');
        self.input.write_all(&request).await?;
        loop {
            let response = self.read().await?;
            if response.id != Some(id) {
                return Err("worker response ID mismatch".into());
            }
            if response.error.is_some() {
                return Err("worker failed to transcribe audio".into());
            }
            if let Some(partial) = response.partial {
                progress.send_replace(partial);
            } else {
                return response
                    .text
                    .ok_or_else(|| "worker response missing text".into());
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Transcriber {
    pub fn start() -> Self {
        let python = env::var("TRANSCRIBER_PYTHON").unwrap_or_else(|_| "python3".into());
        let script =
            env::var("TRANSCRIBER_WORKER").unwrap_or_else(|_| "transcriber/worker.py".into());
        Self::start_with(python, script, Duration::from_mins(15))
    }

    fn start_with(python: String, script: String, job_timeout: Duration) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Job>(4);
        tokio::spawn(async move {
            let mut worker: Option<Worker> = None;
            let mut id = 0_u64;
            while let Some(job) = receiver.recv().await {
                id = id.wrapping_add(1);
                let result = async {
                    if worker.is_none() {
                        worker = Some(Worker::start(&python, &script).await?);
                    }
                    let current = worker.as_mut().ok_or("worker unavailable")?;
                    timeout(
                        job_timeout,
                        current.transcribe(id, job.dir.path().join("voice.ogg"), &job.progress),
                    )
                    .await?
                }
                .await;
                if result.is_err() {
                    worker = None;
                }
                let _ = job
                    .response
                    .send(result.map_err(|error: Error| error.to_string()));
            }
        });
        Self {
            sender,
            slots: Arc::new(Semaphore::new(4)),
        }
    }

    pub async fn handle(&self, bot: &Bot, msg: &Message) -> Result<(), Error> {
        let Some(voice) = msg.voice() else {
            return Ok(());
        };
        if u64::from(voice.file.size) > MAX_BYTES || voice.duration.seconds() > MAX_SECONDS {
            bot.send_message(
                msg.chat.id,
                "El audio es demasiado largo o grande para transcribir.",
            )
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
            return Ok(());
        }
        let Ok(permit) = Arc::clone(&self.slots).try_acquire_owned() else {
            bot.send_message(
                msg.chat.id,
                "Hay demasiadas transcripciones pendientes. Inténtalo más tarde.",
            )
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
            return Ok(());
        };
        let dir = tempfile::tempdir()?;
        download(bot, voice, &dir).await?;
        let status = bot
            .send_message(msg.chat.id, "Transcribiendo…")
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
        let (response, mut result) = oneshot::channel();
        let (progress, progress_rx) = watch::channel(String::new());
        self.sender
            .send(Job {
                dir,
                response,
                progress,
                _permit: permit,
            })
            .await?;
        let mut interval = tokio::time::interval(Duration::from_secs(4));
        let mut displayed = String::new();
        let result = loop {
            tokio::select! {
                result = &mut result => break result,
                _ = interval.tick() => {
                    if let Err(err) = bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing).await {
                        warn!(%err, "could not send typing indicator");
                    }
                    let partial: String = progress_rx.borrow().chars().take(2000).collect();
                    if !partial.is_empty() && partial != displayed {
                        match bot.edit_message_text(msg.chat.id, status.id, &partial).await {
                            Ok(_) => displayed = partial,
                            Err(err) => warn!(%err, "could not update transcript"),
                        }
                    }
                }
            }
        };
        match result {
            Ok(Ok(text)) if !text.is_empty() => {
                let chars: Vec<char> = text.chars().collect();
                for (index, chunk) in chars.chunks(2000).enumerate() {
                    let chunk: String = chunk.iter().collect();
                    if index == 0 {
                        if chunk != displayed {
                            bot.edit_message_text(msg.chat.id, status.id, chunk).await?;
                        }
                    } else {
                        bot.send_message(msg.chat.id, chunk)
                            .reply_parameters(ReplyParameters::new(msg.id))
                            .await?;
                    }
                }
            }
            Ok(Ok(_)) => {
                bot.edit_message_text(msg.chat.id, status.id, "No se ha detectado voz.")
                    .await?;
            }
            Ok(Err(err)) => {
                warn!(%err, "transcription failed");
                bot.edit_message_text(msg.chat.id, status.id, "No he podido transcribir el audio.")
                    .await?;
            }
            Err(err) => {
                warn!(%err, "transcription worker stopped");
                bot.edit_message_text(msg.chat.id, status.id, "No he podido transcribir el audio.")
                    .await?;
            }
        }
        Ok(())
    }
}

async fn download(
    bot: &Bot,
    voice: &teloxide::types::Voice,
    dir: &tempfile::TempDir,
) -> Result<(), Error> {
    timeout(Duration::from_secs(60), async {
        let file = bot.get_file(voice.file.id.clone()).await?;
        let mut out = tokio::fs::File::create(dir.path().join("voice.ogg")).await?;
        let stream = bot.download_file_stream(&file.path);
        futures_util::pin_mut!(stream);
        let mut bytes = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            bytes = bytes
                .checked_add(chunk.len() as u64)
                .ok_or("audio too large")?;
            if bytes > MAX_BYTES {
                return Err::<(), Error>("audio too large".into());
            }
            out.write_all(&chunk).await?;
        }
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restarts_worker_after_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("worker.py");
        std::fs::write(
            &script,
            r"import json, sys, time
from pathlib import Path
marker = Path(__file__).with_suffix('.marker')
print(json.dumps({'ready': True}), flush=True)
for line in sys.stdin:
 r = json.loads(line)
 if not marker.exists():
  marker.touch()
  time.sleep(10)
 print(json.dumps({'id': r['id'], 'text': 'recovered'}), flush=True)
",
        )
        .unwrap();
        let transcriber = Transcriber::start_with(
            "python3".into(),
            script.to_str().unwrap().into(),
            Duration::from_millis(250),
        );
        for should_succeed in [false, true] {
            let (response, result) = oneshot::channel();
            let (progress, _) = watch::channel(String::new());
            let permit = Arc::clone(&transcriber.slots)
                .acquire_owned()
                .await
                .unwrap();
            transcriber
                .sender
                .send(Job {
                    dir: tempfile::tempdir().unwrap(),
                    response,
                    progress,
                    _permit: permit,
                })
                .await
                .unwrap();
            let result = result.await.unwrap();
            assert_eq!(result.is_ok(), should_succeed);
            if should_succeed {
                assert_eq!(result.unwrap(), "recovered");
            }
        }
    }

    #[tokio::test]
    async fn rejects_wrong_response_id() {
        let program = r#"import sys,json
print(json.dumps({"ready": True}), flush=True)
for line in sys.stdin:
 print(json.dumps({"id":999,"text":"wrong"}),flush=True)
"#;
        let mut command = Command::new("python3");
        command.args(["-u", "-c", program]);
        let mut worker = Worker::spawn(&mut command).await.unwrap();
        let (progress, _) = watch::channel(String::new());
        assert!(
            worker
                .transcribe(1, PathBuf::from("voice"), &progress)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn worker_reuses_process_and_matches_responses() {
        let program = r#"import sys,json
print(json.dumps({"ready": True}), flush=True)
for line in sys.stdin:
 r=json.loads(line)
 print(json.dumps({"id":r["id"],"partial":"hola"}),flush=True)
 print(json.dumps({"id":r["id"],"text":str(r["id"])}),flush=True)
"#;
        let mut command = Command::new("python3");
        command.args(["-u", "-c", program]);
        let mut worker = Worker::spawn(&mut command).await.unwrap();
        let (progress, progress_rx) = watch::channel(String::new());
        assert_eq!(
            worker
                .transcribe(1, PathBuf::from("first"), &progress)
                .await
                .unwrap(),
            "1"
        );
        assert_eq!(&*progress_rx.borrow(), "hola");
        assert_eq!(
            worker
                .transcribe(2, PathBuf::from("second"), &progress)
                .await
                .unwrap(),
            "2"
        );
    }
}
