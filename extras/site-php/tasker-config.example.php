<?php
/**
 * Copie este arquivo para `tasker-config.php` e preencha.
 *
 * Só o `tasker-config.php` vai pro htdocs junto com o `tasker.php` — este
 * `.example` é modelo, não precisa subir.
 *
 * Arquivo .php não é servido como texto (o servidor executa), então as senhas
 * daqui não aparecem pra quem abre a URL. Mesmo assim: use uma senha que você não
 * usa em outro lugar, e não versione este arquivo.
 */
return [
    // OBRIGATÓRIO. Sua URL secreta: tasker.php?key=ISTO
    // Gere um valor aleatório, mínimo 16 caracteres. Por exemplo, no seu PC:
    //   python3 -c "import secrets; print(secrets.token_urlsafe(32))"
    'key' => 'troque-isto-por-algo-aleatorio-longo',

    // Caminho A: token do PC (rode `python3 tasker_cloud.py --bootstrap`).
    // Ele ROTACIONA — depois da primeira execução quem manda é o estado gravado,
    // e o valor aqui deixa de ser usado.
    'refresh_token' => '',

    // Caminho B (recomendado): número + senha do portal. Com os dois, se a sessão
    // morrer de vez ele refaz do zero e nunca precisa de você.
    'msisdn' => '',
    'password' => '',

    // Quando repor: sobrando isso ou menos (GB).
    'gb_threshold' => 0.85,
    // Força um refill se passar tantos segundos sem nenhum.
    'max_without_refill' => 780,

    // Avisa você quando precisar de ação humana. Sem cadastro: escolha um nome
    // bem aleatório e instale o app ntfy, ou abra https://ntfy.sh/SEU-TOPICO.
    'ntfy_topic' => '',

    // Onde gravar o estado. Vazio = tenta 'tasker-data/' acima do htdocs e,
    // se não der, dentro do htdocs protegido por .htaccess.
    'state_dir' => '',

    // false deixa a resposta só com OK/ATENCAO, sem o log.
    'verbose' => true,
];
