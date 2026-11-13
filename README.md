# Tasker — refill automático da Lidl Connect

Vigia o volume de *Unlimited Refill* da conta Lidl Connect e pede o refill sozinho
antes de acabar. Programa nativo em **Rust**, com janela própria no estilo do
macOS (tema claro e escuro), para **Linux e Windows**. Nada de navegador, nada
de Python: é um executável só.

## Começar

Precisa do [Rust](https://rustup.rs) para compilar (só na primeira vez ou quando
o código mudar). No Linux também valem os pacotes de sempre de X11/Wayland, que
quase todo desktop já tem.

**Linux:** dê dois cliques no `Tasker.sh` (ou rode `./Tasker.sh`): ele compila se
preciso e abre. Em *Ajustes → Sistema*, clique em **Criar atalho**: aparece o
ícone na área de trabalho e no menu.

**Windows:** ponha o `tasker.exe` numa pasta só dele (o `dados/` é criado ao lado)
e dê dois cliques; não precisa instalar nada, nem o Rust. Como o arquivo não é
assinado, o Windows avisa na primeira vez: *Mais informações → Executar assim
mesmo*. Em *Ajustes → Sistema*, **Criar atalho** põe o ícone na área de trabalho e
no menu Iniciar.

**Gerar os dois programas:** no Linux, `./Compilar.sh` deixa `tasker` e
`tasker.exe` na pasta do projeto. Precisa do MinGW uma vez só: `sudo apt install
gcc-mingw-w64-x86-64` e `rustup target add x86_64-pc-windows-gnu`. No próprio
Windows, `cargo build --release` gera `target\release\tasker.exe`.

**Mandar pra outra pessoa:** envie só o `tasker.exe`. **Nunca a pasta `dados/`**:
ela tem a sua sessão e a sua senha. Quem recebe faz login com a própria conta.

Na primeira vez, se não houver sessão salva, ele pede login. Quem vem da versão em
Python continua logado: a pasta `dados/` é a mesma. Veja
[Vindo da versão em Python](#vindo-da-versão-em-python).

São dois processos do mesmo programa: o **serviço** (vigia o volume, sem janela)
e a **janela**.

* **Fechar a janela não para o Tasker**: o serviço segue vigiando em segundo
  plano. Clicar no atalho de novo só abre a janela (ou traz pra frente, se já
  estiver aberta).
* Pra encerrar de vez: *Ajustes → Sistema → Sair*.
* **Iniciar junto com o computador**: chave em *Ajustes → Sistema*. O serviço sobe
  em segundo plano, sem abrir janela.

## Login

Três jeitos, todos pela tela:

| | quando usar |
|---|---|
| **Número e senha** | o normal. Marcando "lembrar", o Tasker refaz o login sozinho quando a sessão cair. |
| **Entrar pelo navegador** | quando a Lidl pede captcha/segundo fator, ou se você não quer guardar senha. Abre o site da Lidl no Chrome/Edge que você já tem (perfil temporário); você loga normalmente e o Tasker pega a sessão e fecha a janela. É o único uso de navegador no programa. |
| **Sessão já salva** | quem vem da versão 1 continua logado: a sessão foi migrada. |

A senha guardada fica em `dados/credenciais.json`, em texto puro, legível só pelo
seu usuário (modo 600 no Linux, ACL no Windows). Como errar senha trava a conta,
o login automático tem trava: espera crescente entre tentativas, no máximo 3
falhas antes de parar por 6 h, e parada total se a Lidl pedir captcha. Os
contadores ficam em disco, então reiniciar não zera a trava.

## A janela

Barra lateral, barra de ferramentas unificada e os três botões de janela, como num
app de macOS. Atalhos: `Ctrl+1…4` trocam de aba, `Ctrl+R` checa agora, `Ctrl+S`
salva os ajustes, `Ctrl+F` filtra o log, `Ctrl+,` abre os ajustes e `Ctrl+W` fecha
a janela.

* **Painel**: o balde de refill (com a marca do limite), a próxima checagem e o
  motivo dela, o ritmo de consumo, os refills de hoje, quanto tempo o último
  levou pra entrar, e o gráfico das últimas 24 h.
* **Histórico**: gráfico de 6 h a 7 dias e a lista dos refills, com o tempo de
  cada um.
* **Log**: ao vivo, com filtro.
* **Ajustes**: todos os ajustes, que valem **na hora** (não precisa reiniciar),
  mais conta, sistema (iniciar com o computador, atalho, ligar/desligar o
  monitor, testar aviso) e aparência (tema; moldura própria ou a do sistema).
* **Pausar** (30 min, 1 h, 2 h, até você voltar): o Tasker fica **mudo**, sem
  renovar token nem pedir refill. Use isso pra entrar na conta pelo celular ou
  pelo site sem os dois se derrubarem, já que a Lidl aceita uma sessão por vez.
* **Checar agora**: fura a espera do próximo ciclo.
* **Teste de estresse da rede** (no Painel): baixa o máximo que a rede aguentar,
  pra conferir se o refill dá conta de consumo pesado. Detalhes abaixo.

### Teste de estresse

Baixa arquivos de 10 GB de servidores feitos pra teste de velocidade (Hetzner em
Nuremberg, Falkenstein e Helsinque, OVH e Tele2), em várias conexões paralelas
espalhadas entre eles, e **joga os bytes fora na memória**: nada vai pro disco e
quase não gasta CPU. Um servidor que falha fica de lado por um minuto e as
conexões seguem pelos outros. Enquanto roda, o cartão mostra a velocidade (atual,
média e pico), quanto já baixou, quantos refills entraram durante o teste e o
volume mais baixo que a conta atingiu. No fim, diz se o refill acompanhou.

**Consome volume de verdade** (é o objetivo), então para sozinho no limite de GB
ou de tempo (padrão 3 GB / 15 min), ou quando o Tasker é encerrado. Os limites, o
número de conexões e uma URL própria (opcional) ficam em *Ajustes → Teste de
estresse*. O monitor precisa estar ligado, porque é ele quem faz o refill.

Dá pra ver no gráfico uma coisa que o teste deixa clara: a Lidl atualiza o volume
**em degraus** (~0,37 GB a cada ~80 s com uso pesado), não continuamente. Por isso,
com uso pesado, o refill pode sair um pouco abaixo do limite: o número reportado
atravessa o limite de uma vez. Se o volume chegar perto de zero nos testes, suba
o limite em *Ajustes → Checagem*.

## Como funciona

A cada checagem, **uma** requisição traz o volume e o estado de cooldown da opção
de refill (consulta GraphQL combinada, pela mesma conexão HTTPS, que fica aberta
entre checagens: ~130 ms por leitura). O refill é pedido quando:

* sobra **≤ o limite** (padrão 0,85 GB), ou
* passou o **tempo máximo sem refill** (padrão 13 min). Com o balde cheio, ele só
  zera o relógio.

Depois de pedir, ele **confirma em segundos** (no intervalo rápido, padrão 15 s),
olhando o efeito: o volume subiu ou a opção entrou em cooldown.

A **checagem adaptativa** mede o ritmo de consumo e, se no ritmo atual o volume vai
cruzar o limite antes da próxima checagem, olha mais cedo. Com download pesado
(já medido: 0,25 GB/min), o refill sai perto do limite em vez de bem abaixo dele.
Com a rede parada, mantém o intervalo normal.

### O que a API da Lidl tem de traiçoeiro

1. **A mutation de refill devolve HTTP 500 mesmo quando funciona.** O botão do
   próprio site toma o mesmo 500. Por isso a resposta não vale como prova: o
   pedido vira *pendente* (gravado em disco, sobrevive a crash) e quem decide é o
   efeito. A mutation **nunca** é repetida às cegas, nem se a conexão cair no
   meio, porque repetir reservaria duas vezes.
2. **Cooldown ilegível = espera.** Se a parte de tarifas não veio na resposta, o
   Tasker não reserva naquele ciclo; tenta de novo em segundos.
3. **O refresh token rotaciona.** Cada renovação mata o anterior, e o novo é
   gravado em disco *antes* de qualquer outra coisa.
4. **Uma sessão por conta.** Logar no celular/site derruba a do Tasker, e
   vice-versa. Por isso existe a pausa, e por isso duas cópias do Tasker com a
   mesma sessão não sobem (há um lock).

## Ajustes

Ficam em `dados/config.json` (a tela grava). Precedência:

```
padrão do código  <  dados/config.json  <  variável de ambiente TASKER_*
```

A variável de ambiente vence de propósito: mexer na tela não desliga um
`TASKER_*` posto num serviço. A tela mostra esses campos travados. A lista
completa, com explicação e limites, está em [`tasker/config.py`](tasker/config.py).

## Linha de comando

```bash
./tasker                      # abre o Tasker (sobe o serviço se preciso + janela)
./tasker --fundo              # só o serviço, sem janela
./tasker --once               # uma checagem e sai (confirma o refill se pedir um)
./tasker --status             # mostra o volume
./tasker --login              # login pelo navegador
./tasker --set-credentials    # guarda número e senha (--forget-credentials apaga)
./tasker --test-notification
./tasker --bootstrap          # mostra o refresh token (pra levar a sessão pra um servidor)
./tasker --atualizar-atalhos  # conserta atalhos que ainda abrem a versão antiga
./tasker --loop               # só o motor, log no terminal (servidor)
```

Com o Tasker já rodando, `--once` vira "checar agora" e `--login` abre o login
pelo serviço que está no ar, então nada briga pela sessão.

**Arquivos de controle** (qualquer script pode criar, dentro de `dados/`):
`tasker.pause` (`{"until": <timestamp>}`, `0` = até apagar), `tasker.stop`
(encerra limpo) e `tasker.poke` (checa agora).

**Avisos**: notificação do sistema, push no celular via [ntfy.sh](https://ntfy.sh)
(tópico nos ajustes) e/ou webhook (POST `{"text": ...}`). Só os importantes:
login caiu, refill falhando 4 vezes seguidas.

## Servidor (sem deixar o PC ligado)

O mesmo programa roda em servidor: `--loop` (fica rodando) ou `--once` (cron). Em
servidor compile só o motor, sem janela: `cargo build --release
--no-default-features`. A configuração vem por variável de ambiente:
`TASKER_REFRESH_TOKEN` (semente, de `--bootstrap`), `TASKER_MSISDN` +
`TASKER_PASSWORD` (aí ele reloga sozinho pra sempre), `TASKER_DATA_DIR` ou
`TASKER_STATE`, e qualquer `TASKER_*` dos ajustes.

Duas regras:

* **Só um lado pode ter a sessão.** Quando o servidor assumir, feche o Tasker do
  PC, e vice-versa.
* **O estado tem que persistir** entre execuções (é onde mora o token
  rotacionado). Host que apaga o disco quebra a corrente.

**VM / Raspberry Pi** (systemd):

```ini
[Service]
ExecStart=/opt/tasker/tasker --loop
Environment=TASKER_DATA_DIR=/var/lib/tasker
Environment=TASKER_REFRESH_TOKEN=cole-aqui
#Environment=TASKER_MSISDN=...  TASKER_PASSWORD=...  TASKER_NTFY_TOPIC=...
Restart=always
StateDirectory=tasker
```

**Docker**: `docker build -t tasker .` e
`docker run -d --restart unless-stopped -v tasker-state:/data -e TASKER_REFRESH_TOKEN=... tasker`.
A imagem tem ~20 MB. O volume `/data` **não é opcional**. Mantém o
`cloud_state.json` da versão antiga.

**GitHub Actions**: [`.github/workflows/tasker.yml`](.github/workflows/tasker.yml)
roda `--once` a cada 5 min, com o estado no cache do Actions (o programa só é
recompilado quando o código muda). Limitações: o cron do
GitHub atrasa, e ele desliga o agendamento depois de 60 dias sem commit.

**Hospedagem PHP** (htdocs): [`extras/site-php/`](extras/site-php/LEIA-ME.md) é
uma implementação à parte, em PHP, pra hospedagem compartilhada.

## Arquivos

| | |
|---|---|
| `Tasker.sh` | compila se preciso e abre (Linux) |
| `Compilar.sh` | gera `tasker` e `tasker.exe` de uma vez (no Linux) |
| `tasker` / `tasker.exe` | o programa, gerado pelo build |
| `src/` | o código: `engine.rs` (o motor), `lidl.rs` (a API), `daemon.rs` (o serviço), `gui/` (a janela), `browser_login.rs`, `stress.rs`, `system.rs`, `config.rs`, `main.rs` (linha de comando) |
| `assets/` | ícone e a fonte Inter (licença OFL) |
| `dados/` | **tudo que é seu**: sessão, senha, ajustes, log, históricos. Pra levar o Tasker pra outro PC, copie a pasta inteira. |
| `tests/` | testes offline (API de mentira) |
| `extras/site-php/` | a versão PHP |
| `Dockerfile`, `.github/` | servidor |
| `backup/` | cópia da versão 1, de antes da reestruturação |

A janela conversa com o serviço por uma API só em `127.0.0.1`, com chave por
execução guardada em `dados/tela.json` (legível só pelo seu usuário).

## Testes

```bash
cargo test                                # 60+ testes, offline, ~3 s
python3 extras/site-php/test_php.py       # a versão PHP (precisa de php-cli)
```

Rodam em pastas temporárias, contra uma API de mentira que imita a de verdade
(token que rotaciona, 500 mentiroso, captcha, conexão derrubada...). Se houver
Chrome/Edge instalado, o login pelo navegador é testado de verdade (headless,
numa página local).

Pra ver as telas sem abrir nada na sua área de trabalho: com
`TASKER_FOTOS=pasta`, a janela percorre as abas nos dois temas, grava um PNG de
cada e fecha (funciona em `xvfb-run`). Use junto de `TASKER_DATA_DIR` pra não
misturar com os seus dados.

## Se der problema

* **Pede login toda hora**: outra coisa está logada na conta (celular, site, um
  servidor com o Tasker). A Lidl só aceita uma sessão. Use a pausa quando for
  usar a conta.
* **"Já está rodando"**: é o lock. O atalho só reabre a janela; pra encerrar, use
  *Sair* nos ajustes ou crie `dados/tasker.stop`.
* **Cantos pretos na janela**: seu desktop está sem compositor. Ligue a *Barra de
  título do sistema* em *Ajustes → Aparência* (ou `TASKER_DECORACAO=sistema`).
* **Refill não sai**: a aba *Log* filtrada por "refill" mostra o motivo (balde
  cheio, cooldown, espera depois de falhas, resposta da API).
* **Login pelo navegador não abre**: precisa de Chrome, Edge, Chromium ou Brave
  instalado. Sem nenhum, use número e senha.

## Vindo da versão em Python

A versão 3 é o mesmo Tasker reescrito em Rust. A pasta `dados/` é lida como está
(sessão, senha, ajustes, históricos), e o lock é o mesmo, então as duas nunca
giram a sessão juntas.

Se o serviço em Python ainda estiver rodando, a janela nova conecta nele e mostra
**Trocar agora** no Painel: um clique encerra o antigo e sobe o novo, sem login. Os
atalhos e o "iniciar com o computador" que apontavam pro `Tasker.pyw` passam a
abrir o programa novo sozinhos (ou com `./tasker --atualizar-atalhos`).

## O que mudou da versão 1

A v1 mantinha um Chromium escondido (Playwright, ~300 MB) só pra fazer as chamadas
de dentro da página. Da v2 em diante o Tasker fala HTTP direto com a API. Com isso
foram embora o watchdog de navegador travado, o keep-alive do portal, os cookies e
o relogin no Contao. Os ajustes que só existiam por causa do navegador (keep-alive,
headless, reabrir navegador) somem do `config.json` na próxima gravação; o resto
vale como estava. A versão cloud (`tasker_cloud.py`) virou o modo `--loop`/`--once`
do mesmo programa.
