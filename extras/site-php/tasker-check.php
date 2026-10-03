<?php
/**
 * Diagnóstico: descobre se ESTE host consegue rodar o Tasker.
 *
 * Suba só este arquivo no htdocs e abra no navegador. Ele responde a pergunta que
 * importa antes de você configurar qualquer coisa: o host deixa fazer conexão de
 * saída? A maioria das hospedagens grátis NÃO deixa, e sem isso nada funciona.
 *
 * Pode apagar depois. Ele não guarda nada e não usa suas credenciais.
 */
declare(strict_types=1);
header('Content-Type: text/plain; charset=utf-8');
date_default_timezone_set('Europe/Berlin');

$ok = [];
$bad = [];

function line(string $label, string $value): void {
    // printf("%-28s") conta BYTES, e acento em UTF-8 gasta 2 — o alinhamento sai
    // torto. Padding por número de caracteres resolve.
    $label .= ':';
    $pad = max(1, 28 - (function_exists('mb_strlen')
        ? mb_strlen($label, 'UTF-8') : strlen($label)));
    echo $label . str_repeat(' ', $pad) . $value . "\n";
}

echo "=== Tasker — diagnóstico do host ===\n\n";

line('PHP', PHP_VERSION);
if (version_compare(PHP_VERSION, '7.4', '<')) {
    $bad[] = 'PHP muito antigo (precisa 7.4+)';
} else {
    $ok[] = 'versão do PHP';
}

line('Tempo máximo de execução', (string) ini_get('max_execution_time') . 's');
line('Funções desabilitadas', ini_get('disable_functions') ?: '(nenhuma)');

$hasCurl = function_exists('curl_init');
$hasFopen = filter_var(ini_get('allow_url_fopen'), FILTER_VALIDATE_BOOLEAN);
line('cURL', $hasCurl ? 'sim' : 'NÃO');
line('allow_url_fopen', $hasFopen ? 'sim' : 'NÃO');
if ($hasCurl || $hasFopen) {
    $ok[] = 'tem como fazer requisição';
} else {
    $bad[] = 'sem cURL e sem allow_url_fopen — impossível falar com a API';
}

line('OpenSSL (https)', extension_loaded('openssl') ? 'sim' : 'NÃO');
if (!extension_loaded('openssl')) {
    $bad[] = 'sem OpenSSL — não dá pra usar https';
}
line('JSON', function_exists('json_encode') ? 'sim' : 'NÃO');

echo "\n--- Conexão de saída (o teste decisivo)\n";

/** Faz um POST curto e devolve [status, erro]. */
function probe(string $url, string $body): array {
    if (function_exists('curl_init')) {
        $ch = curl_init($url);
        curl_setopt_array($ch, [
            CURLOPT_POST => true,
            CURLOPT_POSTFIELDS => $body,
            CURLOPT_HTTPHEADER => ['Content-Type: application/json'],
            CURLOPT_RETURNTRANSFER => true,
            CURLOPT_TIMEOUT => 20,
            CURLOPT_CONNECTTIMEOUT => 10,
        ]);
        $out = curl_exec($ch);
        $status = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
        $err = curl_error($ch);
        curl_close($ch);
        if ($out === false || $status === 0) {
            return [0, $err ?: 'sem resposta'];
        }
        return [$status, ''];
    }
    $ctx = stream_context_create(['http' => [
        'method' => 'POST',
        'header' => "Content-Type: application/json\r\n",
        'content' => $body,
        'timeout' => 20,
        'ignore_errors' => true,
    ]]);
    $out = @file_get_contents($url, false, $ctx);
    if ($out === false) {
        $e = error_get_last();
        return [0, $e['message'] ?? 'falhou'];
    }
    $status = 0;
    foreach ($http_response_header ?? [] as $h) {
        if (preg_match('#^HTTP/\S+\s+(\d+)#', $h, $m)) {
            $status = (int) $m[1];
        }
    }
    return [$status, ''];
}

$dns = gethostbyname('api.lidl-connect.de');
line('DNS api.lidl-connect.de', $dns === 'api.lidl-connect.de' ? 'FALHOU' : $dns);

// Um refresh_token inválido de propósito: só queremos saber se a resposta CHEGA.
[$status, $err] = probe('https://api.lidl-connect.de/api/token', json_encode([
    'grant_type' => 'refresh_token',
    'client_id' => 'lidl',
    'client_secret' => 'lidl',
    'refresh_token' => 'diagnostico-invalido-de-proposito',
]));

if ($status >= 200) {
    line('POST na API da Lidl', "HTTP $status");
    echo "   (400/401 aqui é o esperado: o token é inválido de propósito.\n";
    echo "    O que importa é a resposta ter chegado.)\n";
    $ok[] = 'conexão de saída liberada';
} else {
    line('POST na API da Lidl', "FALHOU — $err");
    $bad[] = 'conexão de saída bloqueada (é o que mata a maioria dos hosts grátis)';
}

echo "\n--- Onde guardar o estado (precisa ficar FORA do htdocs)\n";

$candidates = [
    dirname(__DIR__) . '/tasker-data' => 'acima do htdocs (ideal)',
    __DIR__ . '/tasker-data' => 'dentro do htdocs (aceitável só com .htaccess)',
];
$writable = null;
foreach ($candidates as $dir => $desc) {
    $exists = is_dir($dir);
    if (!$exists) {
        $exists = @mkdir($dir, 0700, true);
    }
    $can = $exists && is_writable($dir);
    line(basename(dirname($dir)) . '/' . basename($dir), ($can ? 'gravável' : 'não dá') . " — $desc");
    if ($can && $writable === null) {
        $writable = $dir;
    }
}
if ($writable !== null) {
    $ok[] = 'tem onde gravar o estado';
} else {
    $bad[] = 'nenhum diretório gravável — sem isso o token não persiste';
}

echo "\n=== VEREDITO ===\n\n";
if (!$bad) {
    echo "Este host parece servir. Passos:\n";
    echo "  1. suba tasker.php e tasker-config.php no htdocs;\n";
    echo "  2. edite tasker-config.php (chave secreta, token e/ou senha);\n";
    echo "  3. aponte um web-cron (cron-job.org) pra sua URL com ?key=SUACHAVE;\n";
    echo "  4. apague este arquivo de diagnóstico.\n";
} else {
    echo "Não vai dar, por causa de:\n";
    foreach ($bad as $b) {
        echo "  * $b\n";
    }
    echo "\nUse o GitHub Actions (não pede cartão) ou uma VM free tier —\n";
    echo "está tudo explicado no CLOUD.md.\n";
}
if ($ok) {
    echo "\nOK: " . implode(', ', $ok) . "\n";
}
