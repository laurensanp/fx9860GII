//! The calculator window: a small local HTTP server serving a page with the
//! LCD and a clickable keypad. No dependencies; one short-lived thread per request.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

use crate::keys::LAYOUT;
use crate::Shared;

pub fn serve(shared: Arc<Shared>, listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let shared = shared.clone();
        thread::spawn(move || {
            let _ = handle(stream, &shared);
        });
    }
}

fn respond(mut s: TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status,
        ctype,
        body.len()
    )?;
    s.write_all(body)
}

fn query(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn handle(stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut content_length = 0usize;
    let mut h = String::new();
    while reader.read_line(&mut h)? > 2 {
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        h.clear();
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length.min(4096)];
        let _ = std::io::Read::read_exact(&mut reader, &mut body);
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let route = path.split('?').next().unwrap_or("/");
    if route != "/bye" {
        shared.last_seen_ms.store(shared.now_ms(), Ordering::SeqCst);
    }
    match route {
        "/" => respond(stream, "200 OK", "text/html; charset=utf-8", page().as_bytes()),
        "/lcd" => {
            let lcd = shared.lcd.lock().unwrap().clone();
            respond(stream, "200 OK", "application/octet-stream", &lcd)
        }
        "/key" => {
            let code = query(&path, "c").and_then(|c| c.parse::<u8>().ok());
            let down = query(&path, "d").as_deref() == Some("1");
            if let Some(c) = code {
                shared.key_events.lock().unwrap().push_back((c, down));
            }
            respond(stream, "204 No Content", "text/plain", b"")
        }
        "/status" => {
            let s = shared.status.lock().unwrap().clone();
            respond(stream, "200 OK", "text/plain; charset=utf-8", s.as_bytes())
        }
        "/turbo" => {
            let on = query(&path, "on").as_deref() == Some("1");
            shared.turbo.store(on, Ordering::SeqCst);
            respond(stream, "204 No Content", "text/plain", b"")
        }
        "/reset" => {
            shared.reset.store(true, Ordering::SeqCst);
            respond(stream, "204 No Content", "text/plain", b"")
        }
        "/bye" => {
            shared.bye_ms.store(shared.now_ms(), Ordering::SeqCst);
            respond(stream, "204 No Content", "text/plain", b"")
        }
        "/quit" => {
            shared.quit.store(true, Ordering::SeqCst);
            respond(stream, "204 No Content", "text/plain", b"")
        }
        _ => respond(stream, "404 Not Found", "text/plain", b"not found"),
    }
}

fn page() -> String {
    let mut keys = String::new();
    for row in LAYOUT {
        keys += &format!("<div class=\"row r{}\">", row.len());
        for k in row.iter() {
            let cls = match k.name {
                "F1" | "F2" | "F3" | "F4" | "F5" | "F6" => "k fk",
                "SHIFT" => "k shift",
                "ALPHA" => "k alpha",
                "EXE" | "DEL" | "ACON" => "k act",
                "LEFT" | "UP" | "DOWN" | "RIGHT" => "k arrow",
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "DOT" => "k num",
                _ => "k",
            };
            keys += &format!(
                "<button class=\"{}\" data-c=\"{}\" data-n=\"{}\">{}</button>",
                cls, k.code, k.name, k.label
            );
        }
        keys += "</div>";
    }
    PAGE.replace("{{KEYS}}", &keys)
}

const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>fx-9860GII</title>
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><rect x='2' y='0.5' width='12' height='15' rx='2' fill='%232b3a4a'/><rect x='3.5' y='2' width='9' height='4.5' fill='%23c6d3b0'/><g fill='%23e3e7eb'><rect x='3.5' y='8' width='2.5' height='2'/><rect x='6.75' y='8' width='2.5' height='2'/><rect x='10' y='8' width='2.5' height='2'/><rect x='3.5' y='11.5' width='2.5' height='2'/><rect x='6.75' y='11.5' width='2.5' height='2'/></g><rect x='10' y='11.5' width='2.5' height='2' fill='%232f6db3'/></svg>">
<style>
:root { --body:#1d2733; --key:#3c4d60; --keyt:#e8edf2; --lcd:#c6d3b0; }
* { box-sizing: border-box; }
html, body { height: 100%; }
body { margin:0; display:flex; align-items:center; justify-content:center; background:#11161c;
  font-family: system-ui, sans-serif; color:#cfd8e3; user-select:none; }
.calc { background:var(--body); padding:16px 16px 14px; border-radius:22px; width:min(400px, 100vw - 12px);
  box-shadow: 0 10px 40px #0008, inset 0 1px 0 #ffffff14; }
.top { display:flex; align-items:center; justify-content:space-between; margin:0 2px 10px; }
.brand { font-size:11px; letter-spacing:.2em; color:#9fb0c4; }
.tools { display:flex; gap:6px; }
.tool { background:#2b3a4a; color:#b8c6d6; border:0; border-radius:6px; font-size:11px; padding:4px 8px; cursor:pointer; }
.tool:hover { background:#3a4c60; color:#fff; }
.tool.on { background:#c9a227; color:#1a1a1a; }
.screen { background:#0d1218; padding:10px; border-radius:10px; }
canvas { width:100%; image-rendering: pixelated; display:block; background:var(--lcd); border-radius:3px; }
.keys { margin-top:14px; display:flex; flex-direction:column; gap:7px; }
.row { display:grid; gap:7px; }
.r6 { grid-template-columns: repeat(6, 1fr); }
.r5 { grid-template-columns: repeat(5, 1fr); }
.k { background:var(--key); color:var(--keyt); border:0; border-radius:7px; padding:9px 0; font-size:12px;
  cursor:pointer; box-shadow: 0 2px 0 #0007; touch-action:none; }
.k:active, .k.on { transform: translateY(1px); box-shadow:none; filter:brightness(1.3); }
.fk { background:#56677a; font-size:11px; padding:6px 0; }
.shift { background:#c9a227; color:#1a1a1a; }
.alpha { background:#b5423a; }
.num { background:#e3e7eb; color:#1b2430; font-size:15px; font-weight:600; }
.act { background:#2f6db3; }
.arrow { background:#46566a; }
.status { font-size:11px; color:#6f7f93; margin-top:10px; text-align:center; min-height:1em; }
.status b { color:#e0a040; font-weight:600; }
</style></head><body>
<div class="calc">
  <div class="top">
    <span class="brand">CASIO fx-9860GII</span>
    <span class="tools">
      <button class="tool" id="turbo" title="Run as fast as possible (e.g. for slow graphs)">Turbo</button>
      <button class="tool" id="shot" title="Save the screen as a PNG image">Screenshot</button>
      <button class="tool" id="reset" title="Restart the calculator (your files are kept)">Restart</button>
    </span>
  </div>
  <div class="screen"><canvas id="lcd" width="128" height="64"></canvas></div>
  <div class="keys">{{KEYS}}</div>
  <div class="status" id="status"></div>
</div>
<script>
const cv = document.getElementById('lcd'), ctx = cv.getContext('2d');
const img = ctx.createImageData(128, 64);
let offline = false;
async function poll() {
  try {
    const r = await fetch('/lcd'); const b = new Uint8Array(await r.arrayBuffer());
    for (let y = 0; y < 64; y++) for (let x = 0; x < 128; x++) {
      const on = (b[y*16 + (x>>3)] >> (7 - (x&7))) & 1, i = (y*128+x)*4;
      img.data[i] = on ? 28 : 198; img.data[i+1] = on ? 36 : 211; img.data[i+2] = on ? 24 : 176; img.data[i+3] = 255;
    }
    ctx.putImageData(img, 0, 0);
    offline = false;
  } catch (e) { offline = true; }
  setTimeout(poll, 40);
}
async function stat() {
  const el = document.getElementById('status');
  if (offline) { el.innerHTML = '<b>The emulator has stopped. Close this window and start it again.</b>'; }
  else { try { el.textContent = await (await fetch('/status')).text(); } catch (e) {} }
  setTimeout(stat, 1000);
}
poll(); stat();
// Key events go out strictly in order, so a release never overtakes its press.
let queue = Promise.resolve();
function send(c, d) {
  queue = queue.then(() => fetch('/key?c=' + c + '&d=' + (d ? 1 : 0))).catch(() => {});
}
document.querySelectorAll('.k').forEach(b => {
  const c = b.dataset.c;
  b.addEventListener('pointerdown', e => { e.preventDefault(); try { b.setPointerCapture(e.pointerId); } catch (x) {} b.classList.add('on'); send(c, 1); });
  const up = () => { if (b.classList.contains('on')) { b.classList.remove('on'); send(c, 0); } };
  b.addEventListener('pointerup', up); b.addEventListener('pointercancel', up);
});
const km = { '0':'0','1':'1','2':'2','3':'3','4':'4','5':'5','6':'6','7':'7','8':'8','9':'9',
  'Enter':'EXE','Backspace':'DEL','Escape':'EXIT','ArrowUp':'UP','ArrowDown':'DOWN','ArrowLeft':'LEFT','ArrowRight':'RIGHT',
  'F1':'F1','F2':'F2','F3':'F3','F4':'F4','F5':'F5','F6':'F6','m':'MENU','M':'MENU','Shift':'SHIFT','Alt':'ALPHA',
  '+':'ADD','-':'SUB','*':'MUL','/':'DIV','(':'LEFTP',')':'RIGHTP','.':'DOT',',':'COMMA','^':'POWER','Home':'ACON',
  'Delete':'ACON','o':'OPTN','v':'VARS','e':'EXP','x':'XOT','s':'SIN','c':'COS','t':'TAN','l':'LN' };
function keyEl(e) { const n = km[e.key]; return n && document.querySelector('.k[data-n="' + n + '"]'); }
addEventListener('keydown', e => { if (e.ctrlKey || e.metaKey) return; const b = keyEl(e); if (!b) return; e.preventDefault(); if (e.repeat) return; b.classList.add('on'); send(b.dataset.c, 1); });
addEventListener('keyup', e => { const b = keyEl(e); if (!b) return; e.preventDefault(); b.classList.remove('on'); send(b.dataset.c, 0); });
// Toolbar
const turbo = document.getElementById('turbo');
turbo.onclick = () => { const on = !turbo.classList.contains('on'); turbo.classList.toggle('on', on); fetch('/turbo?on=' + (on ? 1 : 0)); };
document.getElementById('reset').onclick = () => {
  if (confirm('Restart the calculator? Your saved files are kept; unsaved work in the current app is lost.')) fetch('/reset');
};
document.getElementById('shot').onclick = () => {
  const s = document.createElement('canvas'); s.width = 128 * 4; s.height = 64 * 4;
  const c2 = s.getContext('2d'); c2.imageSmoothingEnabled = false; c2.drawImage(cv, 0, 0, s.width, s.height);
  const a = document.createElement('a');
  const t = new Date(), p = n => String(n).padStart(2, '0');
  a.download = 'fx9860-' + t.getFullYear() + p(t.getMonth()+1) + p(t.getDate()) + '-' + p(t.getHours()) + p(t.getMinutes()) + p(t.getSeconds()) + '.png';
  a.href = s.toDataURL('image/png'); a.click();
};
// Tell the emulator when the window closes, so it can save and quit.
addEventListener('pagehide', () => { navigator.sendBeacon('/bye'); });
</script></body></html>
"#;
