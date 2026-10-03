<?php
/**
 * Tasker PHP — refill da Lidl Connect em hospedagem compartilhada (htdocs).
 *
 * Não é um site: é um "worker" que roda um ciclo por requisição. Você aponta um
 * web-cron (cron-job.org e afins) pra
 *
 *     https://seusite.tld/tasker.php?key=SUA_CHAVE
 *
 * e cada visita faz uma checagem: lê o volume e, se estiver baixo, dispara o
 * refill. Sem navegador, sem dependência, só PHP + https.
 *
 * Três decisões que valem explicar:
 *
 * 1) O refresh token ROTACIONA a cada uso. O estado é gravado de forma atômica
 *    logo depois do refresh, e fica FORA do htdocs — se ficasse dentro, qualquer
 *    um baixaria seu token pela URL.
 *
 * 2) Requisição web não pode dormir 20 s esperando o refill "aparecer" (tem limite
 *    de tempo de execução). Então a gente marca a reserva como pendente e confirma
 *    na chamada SEGUINTE, olhando se o volume subiu ou se a opção entrou em
 *    cooldown. A API devolve HTTP 500 mesmo quando registra a reserva (o botão do
 *    site faz igual), então confirmar pelo efeito não é luxo, é necessidade.
 *
 * 3) Web-cron pode disparar duas vezes junto. Com o refresh token rotacionando,
 *    duas execuções simultâneas se derrubariam — por isso tem flock.
 *
 * Configure em tasker-config.php (copie de tasker-config.example.php).
 */
declare(strict_types=1);
header('Content-Type: text/plain; charset=utf-8');
date_default_timezone_set('Europe/Berlin');
@set_time_limit(60);

const API_ORIGIN   = 'https://api.lidl-connect.de';
const TOKEN_URL    = API_ORIGIN . '/api/token';
const GRAPHQL_URL  = API_ORIGIN . '/api/graphql';
const CLIENT_ID    = 'lidl';
const CLIENT_SECRET = 'lidl';
const REFILL_OPTION = 'CCS_92061';
const UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) '
         . 'Chrome/148.0.0.0 Safari/537.36';

$defaults = [
    'key' => '',
    'refresh_token' => '',
    'msisdn' => '',
    'password' => '',
    'gb_threshold' => 0.85,
    'max_without_refill' => 780,
    'token_margin' => 300,
    'login_backoff' => 300,
    'login_max_fails' => 3,
    'login_block_long' => 21600,
    'ntfy_topic' => '',
    'state_dir' => '',
    'verbose' => true,
];
$configFile = __DIR__ . '/tasker-config.php';
$CFG = is_file($configFile) ? array_merge($defaults, require $configFile) : $defaults;

// ---------------------------------------------------------------- porta de entrada

/** Responde igual pra chave errada e pra arquivo inexistente: não dá pistas. */
function deny(): void {
    http_response_code(404);
    echo "Not Found\n";
    exit;
}

$given = (string) ($_GET['key'] ?? $_SERVER['HTTP_X_TASKER_KEY'] ?? '');
if ($CFG['key'] === '' || strlen($CFG['key']) < 16) {
    http_response_code(500);
    echo "Configure uma 'key' com pelo menos 16 caracteres em tasker-config.php.\n";
    exit;
}
if (!hash_equals((string) $CFG['key'], $given)) {
    deny();
}

$LOG = [];
function say(string $msg): void {
    global $LOG;
    $LOG[] = date('H:i:s') . ' ' . $msg;
}

// -------------------------------------------------------------------- estado

/**
 * Escolhe onde guardar o estado. Preferência absoluta: fora do htdocs.
 * Se só sobrar dentro, tranca com .htaccess (e o nome do arquivo leva a chave).
 */
function stateFile(array $cfg): string {
    $tries = [];
    if ($cfg['state_dir'] !== '') {
        $tries[] = $cfg['state_dir'];
    }
    $tries[] = dirname(__DIR__) . '/tasker-data';
    $tries[] = __DIR__ . '/tasker-data';

    foreach ($tries as $dir) {
        if (!is_dir($dir) && !@mkdir($dir, 0700, true)) {
            continue;
        }
        if (!is_writable($dir)) {
            continue;
        }
        // Proteção SEMPRE, esteja o diretório onde estiver. Antes a gente
        // "adivinhava" se ele era servível comparando com __DIR__ — e errava na
        // instalação em subpasta: com o script em htdocs/tools/, o candidato
        // htdocs/tasker-data fica fora de __DIR__ mas DENTRO do document root,
        // e o state.json (com o refresh token = a conta inteira) virava URL
        // pública. .htaccess + index.html + nome derivado da chave custam nada
        // fora do webroot e salvam a pele dentro dele (o nome imprevisível
        // também cobre servidor que ignora .htaccess, tipo nginx).
        @file_put_contents($dir . '/.htaccess',
            "Require all denied\n<IfModule !mod_authz_core.c>\nDeny from all\n</IfModule>\n");
        @file_put_contents($dir . '/index.html', '');
        $name = 'state-' . substr(hash('sha256', (string) $cfg['key']), 0, 24) . '.json';
        // Instalações antigas fora do webroot usavam state.json: migra, senão a
        // corrente de refresh tokens ficaria órfã no arquivo velho.
        if (!is_file($dir . '/' . $name) && is_file($dir . '/state.json')) {
            @rename($dir . '/state.json', $dir . '/' . $name);
        }
        return $dir . '/' . $name;
    }
    return '';
}

function loadState(string $file, array $cfg): array {
    $state = [];
    if ($file !== '' && is_file($file)) {
        $raw = @file_get_contents($file);
        $data = $raw === false ? null : json_decode($raw, true);
        if (is_array($data)) {
            $state = $data;
        } else {
            say('estado ilegível — começando do zero');
        }
    }
    // Semente: só usa o token do config se ainda não temos um guardado.
    if (empty($state['refresh']) && $cfg['refresh_token'] !== '') {
        say('usando o refresh_token do config como semente');
        $state['refresh'] = $cfg['refresh_token'];
    }
    return $state;
}

function saveState(string $file, array $state): bool {
    if ($file === '') {
        return false;
    }
    $state['saved_at'] = time();
    $tmp = $file . '.tmp';
    if (@file_put_contents($tmp, json_encode($state, JSON_PRETTY_PRINT)) === false) {
        return false;
    }
    @chmod($tmp, 0600);
    return @rename($tmp, $file);
}

// ---------------------------------------------------------------------- HTTP

/** POST com corpo cru. Devolve ['status' => int, 'body' => string]. */
function post(string $url, string $body, array $headers = []): array {
    $hdr = array_merge([
        'Content-Type' => 'application/json',
        'Accept' => 'application/json, text/plain, */*',
        'Accept-Language' => 'de-DE',
        'User-Agent' => UA,
    ], $headers);

    if (function_exists('curl_init')) {
        $flat = [];
        foreach ($hdr as $k => $v) {
            $flat[] = "$k: $v";
        }
        $ch = curl_init($url);
        curl_setopt_array($ch, [
            CURLOPT_POST => true,
            CURLOPT_POSTFIELDS => $body,
            CURLOPT_HTTPHEADER => $flat,
            CURLOPT_RETURNTRANSFER => true,
            CURLOPT_TIMEOUT => 25,
            CURLOPT_CONNECTTIMEOUT => 10,
        ]);
        $out = curl_exec($ch);
        $status = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
        $err = curl_error($ch);
        curl_close($ch);
        if ($out === false || $status === 0) {
            return ['status' => 0, 'body' => $err ?: 'sem resposta'];
        }
        return ['status' => $status, 'body' => (string) $out];
    }

    $head = '';
    foreach ($hdr as $k => $v) {
        $head .= "$k: $v\r\n";
    }
    $ctx = stream_context_create(['http' => [
        'method' => 'POST', 'header' => $head, 'content' => $body,
        'timeout' => 25, 'ignore_errors' => true,
    ]]);
    $out = @file_get_contents($url, false, $ctx);
    if ($out === false) {
        return ['status' => 0, 'body' => 'falha na requisição'];
    }
    $status = 0;
    foreach ($http_response_header ?? [] as $h) {
        if (preg_match('#^HTTP/\S+\s+(\d+)#', $h, $m)) {
            $status = (int) $m[1];
        }
    }
    return ['status' => $status, 'body' => (string) $out];
}

/** Chama a API GraphQL. Devolve o `data` ou null. */
function api(string $op, string $query, ?array $vars, string $token,
             bool $expect500 = false): ?array {
    $res = post(GRAPHQL_URL, (string) json_encode([
        'operationName' => $op,
        'variables' => $vars ?: new stdClass(),
        'query' => $query,
    ]), ['Authorization' => 'Bearer ' . $token, 'Accept' => '*/*']);

    if ($res['status'] !== 200) {
        if ($res['status'] === 500 && $expect500) {
            say("$op: HTTP 500 genérico — normal aqui, confirmo pelo efeito");
        } else {
            say("$op: HTTP {$res['status']} " . substr(str_replace("\n", ' ', $res['body']), 0, 160));
        }
        return null;
    }
    $body = json_decode($res['body'], true);
    if (!is_array($body)) {
        say("$op: resposta não era JSON");
        return null;
    }
    if (!empty($body['errors'])) {
        $msg = [];
        foreach (array_slice($body['errors'], 0, 2) as $e) {
            $msg[] = substr((string) ($e['message'] ?? '?'), 0, 100);
        }
        say("$op: erro da API: " . implode('; ', $msg));
    }
    return $body['data'] ?? null;
}

// -------------------------------------------------------------------- tokens

function jwtExp(?string $token): float {
    if (!$token || substr_count($token, '.') !== 2) {
        return 0.0;
    }
    $part = explode('.', $token)[1];
    $pad = strlen($part) % 4;
    if ($pad) {
        $part .= str_repeat('=', 4 - $pad);
    }
    $raw = base64_decode(strtr($part, '-_', '+/'), false);
    $data = $raw === false ? null : json_decode($raw, true);
    return is_array($data) ? (float) ($data['exp'] ?? 0) : 0.0;
}

function refreshToken(array &$state, string $file): bool {
    // Tenta o token atual e, se falhar, o anterior: se um ciclo morreu no meio da
    // rotação, o antigo às vezes ainda vale — e isso evita login manual.
    foreach (['refresh' => 'atual', 'refresh_prev' => 'anterior'] as $slot => $label) {
        $rt = $state[$slot] ?? '';
        if (!$rt) {
            continue;
        }
        $res = post(TOKEN_URL, (string) json_encode([
            'grant_type' => 'refresh_token',
            'client_id' => CLIENT_ID,
            'client_secret' => CLIENT_SECRET,
            'refresh_token' => $rt,
        ]));
        if ($res['status'] !== 200) {
            say("refresh ($label) falhou: HTTP {$res['status']} "
                . substr($res['body'], 0, 120));
            continue;
        }
        $body = json_decode($res['body'], true);
        if (!is_array($body) || empty($body['access_token'])) {
            say("refresh ($label): resposta sem access_token");
            continue;
        }
        $access = (string) $body['access_token'];
        $state['refresh_prev'] = $rt;
        $state['refresh'] = (string) ($body['refresh_token'] ?? $rt);
        $state['access'] = $access;
        $state['expires_at'] = jwtExp($access) ?: time() + (float) ($body['expires_in'] ?? 3600);
        // Grava JÁ: o token rotacionou, perder isso custa login manual.
        if (!saveState($file, $state)) {
            say('ATENÇÃO: não consegui gravar o estado — o token novo vai se perder');
        }
        say(sprintf('access token renovado (%.0f min)', ($state['expires_at'] - time()) / 60));
        return true;
    }
    return false;
}

function passwordLogin(array &$state, string $file, array $cfg): bool {
    if ($cfg['msisdn'] === '' || $cfg['password'] === '') {
        return false;
    }
    $fails = (int) ($state['login_fails'] ?? 0);
    $until = (float) ($state['login_blocked_until'] ?? 0);
    if (time() < $until) {
        say(sprintf('login por senha em espera por %.0f min (proteção da conta)',
                    ($until - time()) / 60));
        return false;
    }

    say('tentando login por senha (tentativa ' . ($fails + 1) . ')');
    $res = post(TOKEN_URL, (string) json_encode([
        'grant_type' => 'password',
        'client_id' => CLIENT_ID,
        'client_secret' => CLIENT_SECRET,
        'username' => $cfg['msisdn'],
        'password' => $cfg['password'],
        'captcha_code' => null,
        'captcha_token' => null,
    ]));
    $body = json_decode($res['body'], true);
    $body = is_array($body) ? $body : [];

    if ($res['status'] === 200 && !empty($body['access_token'])) {
        if (!empty($body['needs_multifactor_authentication'])
            && empty($body['mfa_authenticated'])) {
            $state['login_fails'] = 0;
            $state['login_blocked_until'] = time() + (int) $cfg['login_block_long'];
            saveState($file, $state);
            alert($cfg, 'A conta pediu segundo fator: login automático não resolve.');
            return false;
        }
        $access = (string) $body['access_token'];
        $state['access'] = $access;
        $state['refresh'] = (string) ($body['refresh_token'] ?? ($state['refresh'] ?? ''));
        $state['expires_at'] = jwtExp($access) ?: time() + (float) ($body['expires_in'] ?? 3600);
        $state['login_fails'] = 0;
        $state['login_blocked_until'] = 0;
        saveState($file, $state);
        say(sprintf('login por senha OK (%.0f min)', ($state['expires_at'] - time()) / 60));
        return true;
    }

    $text = strtolower($res['body']);
    say("login por senha falhou: HTTP {$res['status']} " . substr($res['body'], 0, 160));
    foreach (['captcha', 'multifactor', 'locked', 'blocked', 'gesperrt', 'lockout'] as $w) {
        if (strpos($text, $w) !== false) {
            // Insistir aqui trava a conta. Melhor parar e chamar o humano.
            $state['login_fails'] = 0;
            $state['login_blocked_until'] = time() + (int) $cfg['login_block_long'];
            saveState($file, $state);
            alert($cfg, 'Login automático bloqueado (captcha ou conta travada).');
            return false;
        }
    }
    $fails++;
    $delay = min((int) $cfg['login_backoff'] * (2 ** ($fails - 1)),
                 (int) $cfg['login_block_long']);
    if ($fails >= (int) $cfg['login_max_fails']) {
        $delay = (int) $cfg['login_block_long'];
        alert($cfg, "Login por senha falhou $fails vezes — segurando "
            . round($delay / 3600) . 'h pra não travar a conta. Confira a senha.');
    }
    $state['login_fails'] = $fails;
    $state['login_blocked_until'] = time() + $delay;
    saveState($file, $state);
    return false;
}

function accessToken(array &$state, string $file, array $cfg): ?string {
    $access = $state['access'] ?? '';
    $expires = (float) ($state['expires_at'] ?? 0);
    if ($access && $expires - (int) $cfg['token_margin'] > time()) {
        return $access;
    }
    if (refreshToken($state, $file)) {
        return $state['access'];
    }
    if ($access && $expires > time()) {
        say('refresh falhou, mas o token atual ainda vale — usando ele');
        return $access;
    }
    if (passwordLogin($state, $file, $cfg)) {
        return $state['access'];
    }
    return null;
}

// -------------------------------------------------------------------- alertas

function alert(array $cfg, string $msg): void {
    say("ALERTA: $msg");
    $topic = trim((string) $cfg['ntfy_topic']);
    if ($topic === '') {
        return;
    }
    $url = strpos($topic, 'http') === 0 ? $topic : "https://ntfy.sh/$topic";
    post($url, $msg, ['Content-Type' => 'text/plain', 'Title' => 'Tasker Lidl']);
}

// ---------------------------------------------------------------------- dados

const Q_CONSUMPTIONS = 'query consumptions { consumptions { consumptionsForUnit '
    . '{ type left max unit __typename } __typename } }';
const Q_TARIFFOPTIONS = 'query tariffOptions { tariffoptions { disabledTariffoptions '
    . '{ disabledTariffoptions { name tariffoptionId } } } }';
const M_REFILL = 'mutation bookTariffOptionsDirect'
    . '($bookTariffoptionsDirectInput: BookTariffoptionsDirectInput!) '
    . '{ bookTariffoptionsDirect(bookTariffoptionsDirectInput: '
    . '$bookTariffoptionsDirectInput) { success __typename } }';

function consumptions(string $token): ?array {
    $data = api('consumptions', Q_CONSUMPTIONS, null, $token);
    $units = $data['consumptions']['consumptionsForUnit'] ?? null;
    if (!is_array($units)) {
        return null;
    }
    $out = [];
    foreach ($units as $u) {
        if (($u['type'] ?? '') === 'REFILLABLE_DATA') {
            $out['refill'] = $u;
        } elseif (($u['type'] ?? '') === 'DATA') {
            $out['data'] = $u;
        }
    }
    return $out;
}

/** true = em cooldown, false = liberado, null = não deu pra saber. */
function refillBlocked(string $token): ?bool {
    $data = api('tariffOptions', Q_TARIFFOPTIONS, null, $token);
    $list = $data['tariffoptions']['disabledTariffoptions']['disabledTariffoptions'] ?? null;
    if (!is_array($list)) {
        return null;
    }
    foreach ($list as $o) {
        if (($o['tariffoptionId'] ?? '') === REFILL_OPTION) {
            return true;
        }
    }
    return false;
}

function bookRefill(string $token): ?bool {
    $data = api('bookTariffOptionsDirect', M_REFILL, [
        'bookTariffoptionsDirectInput' => [
            'bookTariffoptions' => [['tariffoptionId' => REFILL_OPTION]],
        ],
    ], $token, true);
    if ($data === null) {
        return null;
    }
    return !empty($data['bookTariffoptionsDirect']['success']);
}

// ----------------------------------------------------------------- um ciclo

/**
 * Resolve uma reserva pendente da execução anterior.
 * Devolve true se entrou, false se não entrou, null se ainda não dá pra dizer.
 */
function resolvePending(array &$state, string $token, ?float $left): ?bool {
    $pending = $state['pending_refill'] ?? null;
    if (!is_array($pending)) {
        return null;
    }
    $before = isset($pending['before_left']) ? (float) $pending['before_left'] : null;
    $age = time() - (int) ($pending['at'] ?? 0);

    if ($left !== null && $before !== null && $left > $before + 0.005) {
        say(sprintf('reserva anterior entrou: %.2f -> %.2f GB', $before, $left));
        unset($state['pending_refill']);
        return true;
    }
    if (refillBlocked($token) === true) {
        say('reserva anterior em processamento (opção em cooldown)');
        unset($state['pending_refill']);
        return true;
    }
    if ($age < 120) {
        say('reserva anterior ainda sem efeito visível — vejo na próxima');
        return null;
    }
    say('reserva anterior não surtiu efeito');
    unset($state['pending_refill']);
    return false;
}

function cycle(array &$state, string $file, array $cfg): bool {
    $token = accessToken($state, $file, $cfg);
    if ($token === null) {
        alert($cfg, 'Sem token válido: o refresh expirou/foi revogado e o login por '
            . 'senha não resolveu. Gere um token novo com --bootstrap no PC.');
        return false;
    }

    $cons = consumptions($token);
    if ($cons === null) {
        say('não consegui ler o volume neste ciclo');
        return false;
    }

    $refill = $cons['refill'] ?? null;
    $extra = '';
    if (isset($cons['data']['left'])) {
        $extra = ' | plano: ' . (float) $cons['data']['left'] . ' '
               . (string) ($cons['data']['unit'] ?? '');
    }
    if (!$refill) {
        say('nenhum balde REFILLABLE_DATA na conta (tarifa mudou?)' . $extra);
        return true;
    }
    if (($refill['unit'] ?? '') === 'UNLIMITED') {
        say('refill marcado como ilimitado — nada a fazer' . $extra);
        return true;
    }

    $left = isset($refill['left']) ? (float) $refill['left'] : null;
    $max = isset($refill['max']) ? (float) $refill['max'] : null;
    say(sprintf('refill disponível: %.2f/%s GB%s', (float) $left,
                $max === null ? '?' : (string) $max, $extra));

    // Primeiro fecha o assunto da execução anterior.
    $resolved = resolvePending($state, $token, $left);
    if ($resolved === true) {
        $state['last_refill'] = time();
        $state['refill_fails'] = 0;
        saveState($file, $state);
        return true;
    }
    if ($resolved === null && isset($state['pending_refill'])) {
        saveState($file, $state);
        return true;   // espera a próxima chamada pra decidir
    }
    if ($resolved === false) {
        $state['refill_fails'] = (int) ($state['refill_fails'] ?? 0) + 1;
        if ((int) $state['refill_fails'] === 4) {
            alert($cfg, 'O refill falhou 4 vezes seguidas — vale olhar a conta.');
        }
    }

    $lastRefill = (float) ($state['last_refill'] ?? 0);
    $since = $lastRefill > 0 ? time() - $lastRefill : null;
    $forced = $since === null || $since >= (int) $cfg['max_without_refill'];
    $low = $left !== null && $left <= (float) $cfg['gb_threshold'];
    if (!$forced && !$low) {
        say(sprintf('sem refill agora (sobra %.2f GB, último há %.0f min)',
                    (float) $left, ($since ?? 0) / 60));
        saveState($file, $state);
        return true;
    }
    $reason = $forced
        ? ($since === null ? 'primeira execução' : sprintf('timeout (%.0f min)', $since / 60))
        : sprintf('GB baixo (%.2f)', (float) $left);

    if ($left !== null && $max !== null && $left >= $max - 0.005) {
        say("refill pedido ($reason) mas o balde está cheio — nada a fazer");
        $state['last_refill'] = time();
        saveState($file, $state);
        return true;
    }
    if (refillBlocked($token) === true) {
        say("refill pedido ($reason) mas está em cooldown — espero a próxima");
        saveState($file, $state);
        return true;
    }

    say("refill: $reason");
    $ok = bookRefill($token);
    if ($ok === true) {
        say('refill aceito pela API');
        $state['last_refill'] = time();
        $state['refill_fails'] = 0;
        unset($state['pending_refill']);
    } else {
        // 500 (ou success=false) não quer dizer que falhou: confirmamos na próxima.
        say('sem confirmação agora — marco como pendente e confiro na próxima chamada');
        $state['pending_refill'] = ['at' => time(), 'before_left' => $left];
    }
    saveState($file, $state);
    return true;
}

// -------------------------------------------------------------------- execução

$stateFile = stateFile($CFG);
if ($stateFile === '') {
    http_response_code(500);
    echo "Não achei diretório gravável pro estado. Crie 'tasker-data' e dê permissão.\n";
    exit;
}

// Web-cron pode disparar duas vezes junto; com token rotativo isso se derruba.
$lockFile = $stateFile . '.lock';
$lock = @fopen($lockFile, 'c');
if ($lock === false || !flock($lock, LOCK_EX | LOCK_NB)) {
    echo "Já tem uma execução em andamento — saindo.\n";
    exit;
}

$state = loadState($stateFile, $CFG);
if (empty($state['refresh']) && empty($state['access']) && $CFG['msisdn'] === '') {
    http_response_code(500);
    echo "Sem credencial nenhuma: preencha 'refresh_token' ou 'msisdn'+'password'\n"
       . "em tasker-config.php.\n";
    exit;
}

$healthy = false;
try {
    $healthy = cycle($state, $stateFile, $CFG);
} catch (Throwable $e) {
    say('erro inesperado: ' . get_class($e) . ': ' . $e->getMessage());
}
saveState($stateFile, $state);
flock($lock, LOCK_UN);
fclose($lock);

echo ($healthy ? "OK\n" : "ATENCAO\n");
if ($CFG['verbose']) {
    echo implode("\n", $LOG) . "\n";
}
