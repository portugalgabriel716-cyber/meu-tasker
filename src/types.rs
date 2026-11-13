//! O que o serviço manda pra janela (e grava no status.json). Os nomes dos campos
//! são os da versão 2, então o status.json e a API local continuam iguais.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Volume {
    pub left: f64,
    pub total: Option<f64>,
    pub plan_left: Option<f64>,
    pub plan_total: Option<f64>,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub t: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Pending {
    pub at: f64,
    pub before: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LastRefill {
    pub t: f64,
    pub before: Option<f64>,
    pub after: Option<f64>,
    #[serde(default)]
    pub why: String,
    pub latencia: Option<f64>,
}

/// Estado do motor. Os campos depois de `falhas_seguidas` são preenchidos no
/// retrato (`snapshot`), não pelo ciclo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MonitorSnap {
    pub fase: String,
    pub volume: Option<Volume>,
    pub cooldown: Option<bool>,
    pub ritmo_gb_h: Option<f64>,
    pub ultima_checagem: Option<f64>,
    pub proxima_checagem: Option<f64>,
    #[serde(default)]
    pub motivo_intervalo: String,
    pub precisa_login: Option<String>,
    pub erro: Option<String>,
    pub token_expira: Option<f64>,
    pub pendente: Option<Pending>,
    pub ultimo_refill: Option<LastRefill>,
    #[serde(default)]
    pub falhas_seguidas: i64,
    #[serde(default)]
    pub ligado: bool,
    #[serde(default)]
    pub pausado: bool,
    pub pausa_ate: Option<f64>,
    #[serde(default)]
    pub credenciais: bool,
    #[serde(default)]
    pub agora: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refills_hoje: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limite: Option<f64>,
}

impl Default for MonitorSnap {
    fn default() -> Self {
        MonitorSnap {
            fase: "parado".into(),
            volume: None,
            cooldown: None,
            ritmo_gb_h: None,
            ultima_checagem: None,
            proxima_checagem: None,
            motivo_intervalo: String::new(),
            precisa_login: None,
            erro: None,
            token_expira: None,
            pendente: None,
            ultimo_refill: None,
            falhas_seguidas: 0,
            ligado: false,
            pausado: false,
            pausa_ate: None,
            credenciais: false,
            agora: 0.0,
            refills_hoje: None,
            limite: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LoginInfo {
    /// parado | abrindo | aguardando | ok | erro | cancelado
    pub estado: String,
    pub msg: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StressInfo {
    /// parado | rodando | parando
    pub estado: String,
    pub inicio: Option<f64>,
    pub fim: Option<f64>,
    pub segundos: f64,
    pub bytes: u64,
    pub limite_bytes: u64,
    pub limite_segundos: u64,
    pub mbps: f64,
    pub media_mbps: f64,
    pub pico_mbps: f64,
    pub conexoes: i64,
    pub erros: i64,
    pub volume_minimo: Option<f64>,
    #[serde(default)]
    pub motivo: String,
    #[serde(default)]
    pub cfg_gb: f64,
    #[serde(default)]
    pub cfg_min: i64,
    #[serde(default)]
    pub refills: Option<i64>,
    #[serde(default)]
    pub falhas: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SysInfo {
    pub versao: String,
    pub autostart: bool,
    pub navegador: bool,
    pub pasta: String,
    pub windows: bool,
    /// Sobe quando alguém clica no atalho com a janela já aberta: ela vem pra frente.
    #[serde(default)]
    pub foco: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AppState {
    pub monitor: MonitorSnap,
    pub login_navegador: LoginInfo,
    pub estresse: StressInfo,
    pub sistema: SysInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RefillItem {
    pub t: f64,
    pub ok: bool,
    pub before: Option<f64>,
    pub after: Option<f64>,
    #[serde(default)]
    pub why: String,
    pub latencia: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Resumo {
    pub refills_hoje: i64,
    pub refills_periodo: i64,
    pub falhas_periodo: i64,
    pub latencia_media: Option<f64>,
    pub horas: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct History {
    pub volume: Vec<[f64; 2]>,
    pub refills: Vec<RefillItem>,
    pub resumo: Resumo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub help: String,
    pub group: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub env: String,
    pub unit_label: String,
    pub origem: String,
    pub valor: Value,
    pub padrao: Value,
    pub min: Option<Value>,
    pub max: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SettingsPayload {
    pub grupos: Vec<String>,
    pub campos: Vec<Field>,
    pub avisos: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ActionResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub erro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub erros: Option<std::collections::BTreeMap<String, String>>,
}

impl ActionResult {
    pub fn msg(m: impl Into<String>) -> ActionResult {
        ActionResult { msg: Some(m.into()), ..Default::default() }
    }

    pub fn ok(ok: bool, m: impl Into<String>) -> ActionResult {
        ActionResult { ok: Some(ok), msg: Some(m.into()), ..Default::default() }
    }

    pub fn text(&self) -> String {
        self.msg.clone().or_else(|| self.erro.clone()).unwrap_or_default()
    }
}
