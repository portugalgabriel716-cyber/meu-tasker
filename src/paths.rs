//! Onde mora cada arquivo. O programa fica na pasta do projeto; tudo que é estado
//! fica em `dados/`.
//!
//! `TASKER_DATA_DIR` troca a pasta de dados inteira (o Docker usa /data, os
//! testes usam uma pasta temporária). `TASKER_STATE` aponta só o arquivo de
//! sessão — existe pela compatibilidade com a versão cloud (cloud_state.json).
//! `TASKER_CONFIG` aponta outro arquivo de ajustes.
//!
//! Os nomes são os mesmos da versão 2 (Python): a pasta `dados/` dela vale como
//! está, com a sessão e tudo.
use std::path::{Path, PathBuf};

use crate::util::{env_get, Env};

#[derive(Debug, Clone)]
pub struct Paths {
    pub base_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_file: PathBuf,
    /// O lock é por arquivo de sessão: o que não pode existir em dobro é quem
    /// gira a corrente de refresh tokens.
    pub lock_file: PathBuf,
    pub credentials_file: PathBuf,
    pub log_file: PathBuf,
    /// Porta + chave do serviço local (a janela e a linha de comando acham o
    /// Tasker que está rodando por aqui).
    pub ui_file: PathBuf,
    /// Retrato do último ciclo.
    pub status_file: PathBuf,
    /// `{"until": ts}`; 0 = até retomarem.
    pub pause_file: PathBuf,
    /// Existir = encerrar limpo.
    pub stop_file: PathBuf,
    /// Existir = checar agora.
    pub poke_file: PathBuf,
    pub refills_file: PathBuf,
    pub volume_file: PathBuf,
    /// Preferências só da janela (aparência, tamanho).
    pub window_file: PathBuf,
    /// Uma janela só por vez.
    pub window_lock: PathBuf,
}

/// Pasta do programa: a do executável. Rodando de dentro de `target/` (cargo
/// build/run), é a do projeto — assim `dados/` é a mesma de sempre.
pub fn base_dir() -> PathBuf {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let mut cur = dir.as_path();
    // target/release, target/debug, target/<triplo>/release...
    for _ in 0..3 {
        if cur.file_name().is_some_and(|n| n == "target") {
            if let Some(project) = cur.parent() {
                if project.join("Cargo.toml").exists() {
                    return project.to_path_buf();
                }
            }
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }
    dir
}

impl Paths {
    pub fn from_env(env: &Env) -> Paths {
        let base = base_dir();
        let data = env_get(env, "TASKER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.join("dados"));
        Paths::build(base, data, env)
    }

    /// Pasta de dados explícita (testes). `TASKER_STATE`/`TASKER_CONFIG` ainda valem.
    pub fn with_data_dir(data_dir: impl Into<PathBuf>, env: &Env) -> Paths {
        Paths::build(base_dir(), data_dir.into(), env)
    }

    fn build(base_dir: PathBuf, data_dir: PathBuf, env: &Env) -> Paths {
        let config_file = env_get(env, "TASKER_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("config.json"));
        let state_file = env_get(env, "TASKER_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("sessao.json"));
        let stem = state_file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sessao".into());
        let lock_file = state_file.with_file_name(format!("{stem}.lock"));
        let d = |name: &str| data_dir.join(name);
        Paths {
            credentials_file: d("credenciais.json"),
            log_file: d("tasker.log"),
            ui_file: d("tela.json"),
            status_file: d("status.json"),
            pause_file: d("tasker.pause"),
            stop_file: d("tasker.stop"),
            poke_file: d("tasker.poke"),
            refills_file: d("refills.jsonl"),
            volume_file: d("volume.jsonl"),
            window_file: d("janela.json"),
            window_lock: d("janela.lock"),
            base_dir,
            config_file,
            state_file,
            lock_file,
            data_dir,
        }
    }

    pub fn is_default_data_dir(&self) -> bool {
        self.data_dir == self.base_dir.join("dados")
    }

    pub fn ensure_data_dir(&self) {
        let _ = std::fs::create_dir_all(&self.data_dir);
    }

    /// Traz os arquivos da versão 1 (soltos na raiz) pra `dados/`. Devolve a lista.
    ///
    /// Só move quando o destino ainda não existe: rodar duas vezes não faz nada,
    /// e nunca um arquivo antigo passa por cima de um novo.
    pub fn migrate_legacy(&self, base: Option<&Path>) -> Vec<String> {
        let base = match base {
            Some(b) => b.to_path_buf(),
            None => {
                if !self.is_default_data_dir() {
                    return vec![]; // pasta trocada (Docker, testes): nada a migrar
                }
                self.base_dir.clone()
            }
        };
        self.ensure_data_dir();
        let legacy: [(&str, &PathBuf); 6] = [
            ("session_tokens.json", &self.state_file),
            ("config.json", &self.config_file),
            ("credentials.json", &self.credentials_file),
            ("refill_history.jsonl", &self.refills_file),
            ("volume_history.jsonl", &self.volume_file),
            ("tasker.log", &self.log_file),
        ];
        let mut moved = vec![];
        for (old_name, new) in legacy {
            let old = base.join(old_name);
            if old.exists() && !new.exists() && std::fs::rename(&old, new).is_ok() {
                moved.push(old_name.to_string());
            }
        }
        let old_log1 = base.join("tasker.log.1");
        let new_log1 = self.log_file.with_file_name("tasker.log.1");
        if old_log1.exists() && !new_log1.exists() && std::fs::rename(&old_log1, &new_log1).is_ok() {
            moved.push("tasker.log.1".into());
        }
        moved
    }
}
