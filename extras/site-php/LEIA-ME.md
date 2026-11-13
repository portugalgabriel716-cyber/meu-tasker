# Versão pra htdocs (hospedagem PHP compartilhada)

Não é um site: é um *worker* que faz **um ciclo por requisição**. Um web-cron
externo visita a URL de tempo em tempo, e cada visita checa o volume e repõe se
estiver baixo.

## Antes de tudo: seu host serve?

A maioria das hospedagens grátis **bloqueia conexão de saída**, e sem isso nada
funciona. Descubra em 5 segundos:

1. suba **só** `tasker-check.php` no `htdocs`;
2. abra `https://seusite.tld/tasker-check.php`;
3. leia o veredito no fim.

Se disser que a conexão de saída está bloqueada, pare aqui — não tem contorno.
Use GitHub Actions ou uma VM (veja a seção "Servidor" do [README](../../README.md)).

**Apague o `tasker-check.php` depois.**

## O que subir

| arquivo | onde | pra quê |
|---|---|---|
| `tasker.php` | `htdocs/` | o worker |
| `tasker-config.php` | `htdocs/` | suas configurações (crie do `.example`) |
| `.htaccess` | `htdocs/` | impede baixar o config e o estado |

`tasker-check.php` só pro teste, e depois apague. **Do programa em Rust nada vai pro host** —
o resto do projeto (`tasker`, `src/`, `dados/`) fica no PC. O `test_php.py` também não precisa subir.

## Passo a passo

**1. Gere uma chave secreta** (a URL vira secreta; sem ela, qualquer um dispara
seu refill):

```bash
python3 -c "import secrets; print(secrets.token_urlsafe(32))"
```

**2. Crie o `tasker-config.php`** a partir do `tasker-config.example.php`,
preenchendo a `key` e:

* **`msisdn` + `password`** — recomendado: com os dois ele refaz a sessão sozinho,
  pra sempre; ou
* **`refresh_token`** — pegue com `./tasker --bootstrap` no PC. Só
  que ele *rotaciona*: se o host perder o estado, você precisa gerar outro.

**3. Suba os arquivos** e teste no navegador:

```
https://seusite.tld/tasker.php?key=SUA_CHAVE
```

Deve responder `OK` e o log do ciclo. Sem a chave, responde 404.

**4. Agende.** Se o host tiver cron, aponte pra:

```
curl -s "https://seusite.tld/tasker.php?key=SUA_CHAVE"
```

Se não tiver (o normal no plano grátis), use um web-cron externo grátis —
[cron-job.org](https://cron-job.org), EasyCron e afins — com a URL completa a cada
**5 minutos**.

**5. Confirme** que `https://seusite.tld/tasker-data/` **não** lista nem entrega
arquivo nenhum. Se entregar, seu token está exposto: mova o `tasker-data` pra fora
do `htdocs` e aponte `state_dir` no config.

## Segurança — leia

* **A URL é a senha.** Quem tiver ela dispara refill na sua conta. Use HTTPS e uma
  chave longa. Ela vai aparecer no log de acesso do seu host e no painel do
  web-cron — considere isso.
* **O estado nunca pode ser baixável.** Ele guarda o refresh token. O script tenta
  gravar em `tasker-data/` **acima** do `htdocs`; se não der, grava dentro mas cria
  `.htaccess` negando acesso e usa um nome de arquivo derivado da sua chave. Só que
  `.htaccess` **não funciona no nginx** — nesse caso é obrigatório usar um diretório
  fora do `htdocs`.
* **`.php` não é servido como texto** (o servidor executa), então a senha no
  `tasker-config.php` não aparece pra quem abre a URL — *enquanto o PHP funcionar*.
  Se o PHP quebrar, alguns servidores entregam o fonte. Prefira uma senha que você
  não usa em outro lugar.

## Como ele lida com o limite de tempo

Requisição web não pode dormir 20 s esperando o refill aparecer. Então o refill vai
em duas etapas: reserva agora, e a **chamada seguinte** confirma se o volume subiu
ou se a opção entrou em cooldown. Isso importa porque a API devolve HTTP 500 mesmo
quando registra a reserva (o botão do site faz igual) — sem confirmar pelo efeito,
ele acharia que falhou e reservaria de novo.

Um `flock` impede que dois disparos simultâneos do web-cron rodem juntos: o refresh
token rotaciona, e duas execuções ao mesmo tempo se derrubariam.

## E a regra que vale pra tudo

A Lidl aceita **uma sessão por conta**, e o refresh token rotaciona. Então: ou o
host, ou o Tasker do PC — **nunca os dois**.

## Testes

```bash
python3 tests/test_php.py    # 57 asserções, precisa de php-cli
```

Sobe o servidor embutido do PHP contra uma API falsa e exercita porta de entrada,
rotação de token, login por senha com as travas, e o refill em duas etapas. Não
fala com a Lidl.
