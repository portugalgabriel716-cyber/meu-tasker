//! Log: arquivo com rotação + memória curta numerada (a janela puxa por número).
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::util::{fmt_local, now};

pub const LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogChunk {
    pub seq: u64,
    pub reset: bool,
    pub linhas: Vec<String>,
}

struct Inner {
    lines: VecDeque<(u64, String)>,
    seq: u64,
    quiet: HashMap<String, (String, f64)>,
}

pub struct Logger {
    file: Option<PathBuf>,
    to_stdout: bool,
    date: bool,
    keep: usize,
    inner: Mutex<Inner>,
}

impl Logger {
    pub fn new(file: Option<PathBuf>, to_stdout: bool, date: bool) -> Logger {
        Logger {
            file,
            to_stdout,
            date,
            keep: 600,
            inner: Mutex::new(Inner { lines: VecDeque::new(), seq: 0, quiet: HashMap::new() }),
        }
    }

    /// Só memória (testes).
    pub fn silent() -> Logger {
        Logger::new(None, false, false)
    }

    pub fn log(&self, msg: impl AsRef<str>) {
        let fmt = if self.date { "%Y-%m-%d %H:%M:%S" } else { "%d/%m %H:%M:%S" };
        let line = format!("[{}] {}", fmt_local(now(), fmt), msg.as_ref());
        {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            g.seq += 1;
            let seq = g.seq;
            g.lines.push_back((seq, line.clone()));
            while g.lines.len() > self.keep {
                g.lines.pop_front();
            }
            self.write(&line);
        }
        if self.to_stdout {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    }

    /// Loga, mas não repete a mesma chave+mensagem antes de `every` segundos.
    ///
    /// É o que segura o log quando uma situação dura (espera, cooldown, balde
    /// cheio): sem isso sairia a mesma linha a cada checagem.
    pub fn once(&self, key: &str, msg: impl AsRef<str>, every: f64) {
        let msg = msg.as_ref();
        {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((last, t)) = g.quiet.get(key) {
                if last == msg && now() - t < every {
                    return;
                }
            }
            g.quiet.insert(key.to_string(), (msg.to_string(), now()));
        }
        self.log(msg);
    }

    fn write(&self, line: &str) {
        let Some(f) = &self.file else { return };
        // Log em disco é conveniência; nunca derruba o monitor.
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(f).map(|m| m.len() > LOG_MAX_BYTES).unwrap_or(false) {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = std::fs::rename(f, f.with_file_name(format!("{name}.1")));
        }
        if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
            let _ = writeln!(fh, "{line}");
        }
    }

    /// Linhas depois de `seq`. seq<=0 (ou que já saiu da memória) = tudo.
    pub fn since(&self, seq: u64) -> LogChunk {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let current = g.seq;
        let first = g.lines.front().map(|l| l.0).unwrap_or(0);
        // Número maior que o atual = a janela é de antes de um reinício: manda tudo.
        let reset = seq == 0 || seq > current || (!g.lines.is_empty() && seq + 1 < first);
        let linhas = g
            .lines
            .iter()
            .filter(|(n, _)| reset || *n > seq)
            .map(|(_, l)| l.clone())
            .collect();
        LogChunk { seq: current, reset, linhas }
    }

    /// Traz o fim do tasker.log pra memória (a janela abre já com contexto).
    pub fn preload(&self, n: usize) {
        let Some(path) = &self.file else { return };
        let Ok(mut f) = std::fs::File::open(path) else { return };
        let size = f.seek(SeekFrom::End(0)).unwrap_or(0);
        let start = size.saturating_sub(64 * 1024);
        if f.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        let mut buf = vec![];
        let _ = f.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<&str> = text.lines().collect();
        if start > 0 && !lines.is_empty() {
            lines.remove(0); // a primeira linha veio cortada pelo meio
        }
        let skip = lines.len().saturating_sub(n);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for line in &lines[skip..] {
            g.seq += 1;
            let seq = g.seq;
            g.lines.push_back((seq, line.to_string()));
            while g.lines.len() > self.keep {
                g.lines.pop_front();
            }
        }
    }
}
