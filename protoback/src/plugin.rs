// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Plugins (spec 0431): `--fdp-plugin FILE` through the embedded Python
//! adapter, `--fdp-plugin-cmd CMD` through `sh -c`; either way one
//! process per run, spoken to in frames (S2).
//!
//! A frame is a varint length and that many bytes. protoback writes a
//! serialized `FileDescriptorProto`; the plugin answers with a status byte,
//! `0` and the changed descriptor, or `1` and an error message. It first
//! writes one frame unprompted: `0` alone when ready, `1` and a message
//! when it cannot run.

use crate::hash::HashMap;
use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

/// The adapter `--fdp-plugin` runs (S3).
const ADAPTER: &str = include_str!("plugin/adapter.py");

/// Where the plugin comes from.
pub enum Source {
    /// `--fdp-plugin FILE`, or the deprecated `--phase2-plugin FILE`.
    Python { file: String, legacy: bool },
    /// `--fdp-plugin-cmd CMD`.
    Command(String),
}

struct Process {
    child: Child,
    /// Dropped to close the pipe, which ends the plugin's loop.
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// Each answer, by the bytes sent (S5).
    cache: HashMap<Vec<u8>, Vec<u8>>,
    /// The first failure: the run ends with it (S4).
    failed: Option<String>,
}

/// A running plugin; calls go through a lock (S6).
pub struct Plugin {
    /// What a failure names: the plugin file, or the command.
    label: String,
    process: Mutex<Process>,
}

fn write_frame(w: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    let mut n = bytes.len() as u64;
    let mut varint = Vec::with_capacity(10);
    while n >= 0x80 {
        varint.push((n as u8) | 0x80);
        n >>= 7;
    }
    varint.push(n as u8);
    w.write_all(&varint)?;
    w.write_all(bytes)?;
    w.flush()
}

/// One frame: `None` at the end of the stream, or on a frame cut short.
fn read_frame(r: &mut impl Read) -> Option<Vec<u8>> {
    let mut n: u64 = 0;
    let mut shift = 0;
    loop {
        let mut b = [0u8; 1];
        r.read_exact(&mut b).ok()?;
        if shift > 63 {
            return None;
        }
        n |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    let mut data = vec![0; usize::try_from(n).ok()?];
    r.read_exact(&mut data).ok()?;
    Some(data)
}

/// A status frame: `Ok` with the payload after `0`, `Err` with the message
/// after `1`, `Err` describing anything else.
fn status(frame: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let Some(frame) = frame else {
        return Err("the plugin exited".to_string());
    };
    match frame.split_first() {
        Some((0, payload)) => Ok(payload.to_vec()),
        Some((1, message)) => Err(String::from_utf8_lossy(message).into_owned()),
        Some((s, _)) => Err(format!("malformed frame (status byte {s})")),
        None => Err("malformed frame (empty)".to_string()),
    }
}

impl Plugin {
    /// The plugin started, once it says it is ready; `Err` with the
    /// message the run ends with.
    pub fn start(source: Source) -> Result<Plugin, String> {
        let (label, mut command) = match &source {
            Source::Python { file, legacy } => {
                let python =
                    std::env::var("PROTOBACK_PYTHON").unwrap_or_else(|_| "python3".to_string());
                let mut c = Command::new(python);
                c.arg("-c")
                    .arg(ADAPTER)
                    .arg(file)
                    .arg(if *legacy { "1" } else { "0" });
                (file.clone(), c)
            }
            Source::Command(cmd) => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(cmd);
                (cmd.clone(), c)
            }
        };
        let fail = |what: String| format!("Error: {label}: {what}");
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| fail(format!("cannot start the plugin: {e}")))?;
        let stdin = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().expect("piped"));
        status(read_frame(&mut stdout)).map_err(fail)?;
        Ok(Plugin {
            label: label.clone(),
            process: Mutex::new(Process {
                child,
                stdin,
                stdout,
                cache: HashMap::default(),
                failed: None,
            }),
        })
    }

    /// `apply_fdp_plugin`: the plugin's answer for `bytes`, the
    /// descriptor of file `name`. After a failure, `bytes` unchanged: the
    /// run ends at the next check (`failure`).
    pub fn apply(&self, name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut p = self.process.lock().unwrap_or_else(|e| e.into_inner());
        if p.failed.is_some() {
            return bytes.to_vec();
        }
        if let Some(answer) = p.cache.get(bytes) {
            return answer.clone();
        }
        let p = &mut *p;
        let answer = p
            .stdin
            .as_mut()
            .ok_or(())
            .and_then(|stdin| write_frame(stdin, bytes).map_err(|_| ()))
            .map_err(|()| "the plugin exited".to_string())
            .and_then(|()| status(read_frame(&mut p.stdout)))
            .and_then(|changed| {
                let def = prototext_upb::Pool::vendored()
                    .get_message_by_name("google.protobuf.FileDescriptorProto")
                    .expect("the vendored descriptor.proto declares it");
                match prototext_upb::DynamicMessage::decode(&def, &changed) {
                    Ok(_) => Ok(changed),
                    Err(_) => Err("malformed frame (not a FileDescriptorProto)".to_string()),
                }
            });
        match answer {
            Ok(changed) => {
                p.cache.insert(bytes.to_vec(), changed.clone());
                changed
            }
            Err(what) => {
                // The adapter's errors are reproto's text, naming the
                // file; a command's are named here.
                p.failed = Some(if what.starts_with("plugin raised ") {
                    what
                } else {
                    format!("plugin failed on '{name}': {what}")
                });
                bytes.to_vec()
            }
        }
    }

    /// The failure the run ends with, if the plugin failed:
    /// `Error: FILE: MESSAGE` (reproto's `ClickException`).
    pub fn failure(&self) -> Option<String> {
        let p = self.process.lock().unwrap_or_else(|e| e.into_inner());
        p.failed
            .as_ref()
            .map(|what| format!("Error: {}: {what}", self.label))
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let p = self.process.get_mut().unwrap_or_else(|e| e.into_inner());
        // Closing its stdin ends the plugin's loop; then it is reaped.
        drop(p.stdin.take());
        let _ = p.child.wait();
    }
}
