use std::{env, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use teloxide::{net::Download, prelude::*, types::ReplyParameters};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    time::timeout,
};
use tracing::warn;

const MAX_BYTES: u64 = 20_000_000;
const MAX_SECONDS: u32 = 120;
type Error = Box<dyn std::error::Error + Send + Sync>;

struct Job {
    dir: tempfile::TempDir,
    response: oneshot::Sender<Result<String, String>>,
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

    async fn transcribe(&mut self, id: u64, path: PathBuf) -> Result<String, Error> {
        let mut request = serde_json::to_vec(&Request { id, path })?;
        request.push(b'\n');
        self.input.write_all(&request).await?;
        let response = self.read().await?;
        if response.id != Some(id) {
            return Err("worker response ID mismatch".into());
        }
        if response.error.is_some() {
            return Err("worker failed to transcribe audio".into());
        }
        response
            .text
            .ok_or_else(|| "worker response missing text".into())
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
        Self::start_with(python, script, Duration::from_secs(120))
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
                        current.transcribe(id, job.dir.path().join("voice.ogg")),
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
        .await??;
        let (response, result) = oneshot::channel();
        self.sender
            .send(Job {
                dir,
                response,
                _permit: permit,
            })
            .await?;
        match result.await {
            Ok(Ok(text)) if !text.is_empty() => {
                bot.send_message(msg.chat.id, text)
                    .reply_parameters(ReplyParameters::new(msg.id))
                    .await?;
            }
            Ok(Ok(_)) => (),
            Ok(Err(err)) => {
                warn!(%err, "transcription failed");
                bot.send_message(msg.chat.id, "No he podido transcribir el audio.")
                    .reply_parameters(ReplyParameters::new(msg.id))
                    .await?;
            }
            Err(err) => {
                warn!(%err, "transcription worker stopped");
            }
        }
        Ok(())
    }
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
            let permit = Arc::clone(&transcriber.slots)
                .acquire_owned()
                .await
                .unwrap();
            transcriber
                .sender
                .send(Job {
                    dir: tempfile::tempdir().unwrap(),
                    response,
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
        assert!(worker.transcribe(1, PathBuf::from("voice")).await.is_err());
    }

    #[tokio::test]
    async fn worker_reuses_process_and_matches_responses() {
        let program = r#"import sys,json
print(json.dumps({"ready": True}), flush=True)
for line in sys.stdin:
 r=json.loads(line)
 print(json.dumps({"id":r["id"],"text":str(r["id"])}),flush=True)
"#;
        let mut command = Command::new("python3");
        command.args(["-u", "-c", program]);
        let mut worker = Worker::spawn(&mut command).await.unwrap();
        assert_eq!(
            worker.transcribe(1, PathBuf::from("first")).await.unwrap(),
            "1"
        );
        assert_eq!(
            worker.transcribe(2, PathBuf::from("second")).await.unwrap(),
            "2"
        );
    }
}
