//! Conversa com o serviço que está rodando (a janela e a linha de comando usam).
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::paths::Paths;
use crate::store::read_json;

#[derive(Clone)]
pub struct Client {
    base: String,
    key: String,
    agent: ureq::Agent,
}

impl Client {
    /// Pelo dados/tela.json. None se não há serviço publicado.
    pub fn find(paths: &Paths) -> Option<Client> {
        let info = read_json(&paths.ui_file)?;
        let port = info.get("port")?.as_u64()?;
        let key = info.get("key")?.as_str()?.to_string();
        Some(Client::new(port as u16, key))
    }

    pub fn new(port: u16, key: String) -> Client {
        Client {
            base: format!("http://127.0.0.1:{port}"),
            key,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(2))
                .timeout(Duration::from_secs(30))
                .build(),
        }
    }

    pub fn same_as(&self, other: &Client) -> bool {
        self.base == other.base && self.key == other.key
    }

    fn finish<T: DeserializeOwned>(r: Result<ureq::Response, ureq::Error>) -> Result<T, String> {
        let resp = match r {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                return Err(format!("HTTP {code}: {text}"));
            }
            Err(e) => return Err(e.to_string()),
        };
        resp.into_json::<T>().map_err(|e| format!("resposta inválida: {e}"))
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        Self::finish(self.agent.get(&format!("{}{path}", self.base)).set("X-Tasker-Key", &self.key).call())
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, String> {
        Self::finish(
            self.agent
                .post(&format!("{}{path}", self.base))
                .set("X-Tasker-Key", &self.key)
                .send_json(body.clone()),
        )
    }

    /// O serviço responde?
    pub fn alive(&self) -> bool {
        self.get::<Value>("/api/log?desde=999999999").is_ok()
    }
}
