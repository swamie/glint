//! Discord Rich Presence over Discord's local pipe. Nothing leaves the machine
//! from Glint: the Discord client on this PC is told what to show.
//!
//! Discord only accepts a presence from a registered application, so the user
//! supplies the Application ID of one they created in Discord's developer portal.
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

/// What the presence shows. `None` fields are left out.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Activity {
    pub details: String,
    pub state: String,
    /// Unix seconds: Discord shows the time elapsed since then.
    pub since: Option<u64>,
}

pub struct Client {
    pipe: File,
    nonce: u64,
}

fn write_frame(pipe: &mut File, op: u32, body: &Value) -> std::io::Result<()> {
    let text = body.to_string();
    let mut buf = Vec::with_capacity(8 + text.len());
    buf.extend_from_slice(&op.to_le_bytes());
    buf.extend_from_slice(&(text.len() as u32).to_le_bytes());
    buf.extend_from_slice(text.as_bytes());
    pipe.write_all(&buf)?;
    pipe.flush()
}

fn read_frame(pipe: &mut File) -> std::io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    pipe.read_exact(&mut head)?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::other("Discord sent an oversized reply"));
    }
    let mut body = vec![0u8; len];
    pipe.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

impl Client {
    /// Connect to the running Discord client and identify as `app_id`.
    pub fn connect(app_id: &str) -> Result<Client, String> {
        let app_id = app_id.trim();
        if app_id.is_empty() || !app_id.chars().all(|c| c.is_ascii_digit()) {
            return Err("The Application ID is a number of about 18 digits".into());
        }
        let mut pipe = (0..10)
            .find_map(|i| OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok())
            .ok_or("Discord is not running")?;
        write_frame(&mut pipe, 0, &json!({ "v": 1, "client_id": app_id })).map_err(|e| format!("Discord did not answer: {e}"))?;
        let (op, body) = read_frame(&mut pipe).map_err(|e| format!("Discord did not answer: {e}"))?;
        // Opcode 1 with a READY event is success; opcode 2 is Discord closing the connection.
        if op != 1 || body["evt"] != "READY" {
            let why = body["message"].as_str().or(body["data"]["message"].as_str()).unwrap_or("connection refused");
            return Err(format!("Discord refused the Application ID: {why}"));
        }
        Ok(Client { pipe, nonce: 0 })
    }

    /// Show `activity`, or clear the presence with `None`.
    pub fn set(&mut self, activity: Option<&Activity>) -> Result<(), String> {
        self.nonce += 1;
        let act = activity.map(|a| {
            let mut v = json!({ "details": a.details, "instance": false });
            if !a.state.is_empty() {
                v["state"] = json!(a.state);
            }
            if let Some(t) = a.since {
                v["timestamps"] = json!({ "start": t });
            }
            v
        });
        let msg = json!({ "cmd": "SET_ACTIVITY", "args": { "pid": std::process::id(), "activity": act }, "nonce": self.nonce.to_string() });
        write_frame(&mut self.pipe, 1, &msg).map_err(|e| format!("Discord connection lost: {e}"))?;
        let (op, body) = read_frame(&mut self.pipe).map_err(|e| format!("Discord connection lost: {e}"))?;
        if op != 1 || body["evt"] == "ERROR" {
            return Err(format!("Discord rejected the presence: {}", body["data"]["message"].as_str().unwrap_or("unknown error")));
        }
        Ok(())
    }
}
