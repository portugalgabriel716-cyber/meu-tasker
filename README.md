# Tasker — refill automático da Lidl Connect

Vigia o volume de *Unlimited Refill* da conta Lidl Connect e pede o refill sozinho
antes de acabar. Roda em **Linux e Windows** com um clique, e precisa **só do
Python 3.8+**: nada de `pip install`, nada de navegador escondido.

## Começar

**Linux:** dê dois cliques no `Tasker.sh` (ou rode `python3 Tasker.pyw`). Na aba
*Ajustes → Sistema*, clique em **Criar atalho**: aparece o ícone na área de
trabalho e no menu. Daí em diante é só clicar nele.

**Windows:** instale o Python de [python.org](https://www.python.org/downloads/)
(o instalador padrão já serve) e dê dois cliques no **`Tasker.pyw`**. Em
*Ajustes → Sistema*, **Criar atalho** põe o ícone na área de trabalho e no menu
Iniciar.

O Tasker abre numa janela própria (o Chrome ou Edge que você já tem, em "modo app";
sem nenhum dos dois, abre no navegador padrão). Na primeira vez, se não houver
sessão salva, ele pede login.

* **Fechar a janela não para o Tasker** — ele segue vigiando em segundo plano.
  Clicar no atalho de novo só reabre a janela.
* Pra encerrar de vez: *Ajustes → Sistema → Sair*.
* **Iniciar junto com o computador**: chave em *Ajustes → Sistema*. Ele sobe em
  segundo plano, sem abrir janela.

## Login

Três jeitos, todos pela tela:

| | quando usar |
|---|---|
| **Número e senha** | o normal. Marcando "lembrar", o Tasker refaz o login sozinho quando a sessão cair. |
| **Entrar pelo navegador** | quando a Lidl pede captcha/segundo fator, ou se você não quer guardar senha. Abre o site da Lidl numa janela à parte (perfil temporário); você loga normalmente e o Tasker pega a sessão e fecha a janela. |
| **Sessão já salva** | quem vem da versão 1 continua logado: a sessão foi migrada. |

A senha guardada fica em `dados/credenciais.json`, em texto puro, legível só pelo
seu usuário (modo 600 no Linux, ACL no Windows). Como errar senha trava a conta,
o login automático tem trava: espera crescente entre tentativas, no máximo 3
falhas antes de parar por 6 h, e parada total se a Lidl pedir captcha. Os
contadores ficam em disco, então reiniciar não zera a trava.

## A tela

* **Painel**: o balde de refill (com a marca do limite), a próxima checagem e o
  motivo dela, o ritmo de consumo, os refills de hoje, quanto tempo o último
  levou pra entrar, e o gráfico das últimas 24 h.
* **Histórico**: gráfico de 6 h a 7 dias e a lista dos refills, com o tempo de
  cada um.
* **Log**: ao vivo, com filtro.
* **Ajustes**: todos os ajustes, que valem **na hora** (não precisa reiniciar),
  mais conta e sistema (iniciar com o computador, atalho, ligar/desligar o
  monitor, testar aviso).
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
python3 Tasker.pyw                 # abre o Tasker (ou só a janela, se já estiver rodando)
python3 Tasker.pyw --fundo         # sobe sem janela
python3 Tasker.pyw --once          # uma checagem e sai (confirma o refill se pedir um)
python3 Tasker.pyw --status        # mostra o volume
python3 Tasker.pyw --login         # login pelo navegador
python3 Tasker.pyw --set-credentials | --forget-credentials
python3 Tasker.pyw --test-notification
python3 Tasker.pyw --bootstrap     # mostra o refresh token (pra levar a sessão pra um servidor)
python3 Tasker.pyw --loop          # só o motor, log no terminal (servidor)
```

Com o Tasker já rodando, `--once` vira "checar agora" e `--login` abre o login
pela instância que está no ar, então nada briga pela sessão.

**Arquivos de controle** (qualquer script pode criar, dentro de `dados/`):
`tasker.pause` (`{"until": <timestamp>}`, `0` = até apagar), `tasker.stop`
(encerra limpo) e `tasker.poke` (checa agora).

**Avisos**: notificação do sistema, push no celular via [ntfy.sh](https://ntfy.sh)
(tópico nos ajustes) e/ou webhook (POST `{"text": ...}`). Só os importantes:
login caiu, refill falhando 4 vezes seguidas.

## Servidor (sem deixar o PC ligado)

O mesmo programa roda em servidor: `--loop` (fica rodando) ou `--once` (cron). A
configuração vem por variável de ambiente: `TASKER_REFRESH_TOKEN` (semente, de
`--bootstrap`), `TASKER_MSISDN` + `TASKER_PASSWORD` (aí ele reloga sozinho pra
sempre), `TASKER_DATA_DIR` ou `TASKER_STATE`, e qualquer `TASKER_*` dos ajustes.

Duas regras:

* **Só um lado pode ter a sessão.** Quando o servidor assumir, feche o Tasker do
  PC, e vice-versa.
* **O estado tem que persistir** entre execuções (é onde mora o token
  rotacionado). Host que apaga o disco quebra a corrente.

**VM / Raspberry Pi** (systemd):

```ini
[Service]
ExecStart=/usr/bin/python3 /opt/tasker/Tasker.pyw --loop
Environment=TASKER_DATA_DIR=/var/lib/tasker
Environment=TASKER_REFRESH_TOKEN=cole-aqui
#Environment=TASKER_MSISDN=...  TASKER_PASSWORD=...  TASKER_NTFY_TOPIC=...
Restart=always
StateDirectory=tasker
```

**Docker**: `docker build -t tasker .` e
`docker run -d --restart unless-stopped -v tasker-state:/data -e TASKER_REFRESH_TOKEN=... tasker`.
O volume `/data` **não é opcional**. Mantém o `cloud_state.json` da versão antiga.

**GitHub Actions**: [`.github/workflows/tasker.yml`](.github/workflows/tasker.yml)
roda `--once` a cada 5 min, com o estado no cache do Actions. Limitações: o cron do
GitHub atrasa, e ele desliga o agendamento depois de 60 dias sem commit.

**Hospedagem PHP** (htdocs): [`extras/site-php/`](extras/site-php/LEIA-ME.md) é
uma implementação à parte, em PHP, pra hospedagem compartilhada.

## Arquivos

| | |
|---|---|
| `Tasker.pyw` | o programa (duplo clique no Windows; `python3 Tasker.pyw` em qualquer lugar) |
| `Tasker.sh` | atalho do Linux pro mesmo |
| `tasker/` | o código: `engine.py` (o motor), `lidl.py` (a API), `app.py` (tela + linha de comando), `server.py`, `browser_login.py`, `stress.py` (teste de estresse), `system.py`, `config.py`, `ui/` |
| `dados/` | **tudo que é seu**: sessão, senha, ajustes, log, históricos. Pra levar o Tasker pra outro PC, copie a pasta inteira. |
| `tests/` | testes offline (API de mentira) |
| `extras/site-php/` | a versão PHP |
| `Dockerfile`, `.github/` | servidor |
| `backup/` | cópia da versão 1, de antes da reestruturação |

## Testes

```bash
python3 tests/test_tasker.py              # 60 testes, offline, ~45 s
python3 extras/site-php/test_php.py       # a versão PHP (precisa de php-cli)
```

Rodam numa pasta temporária, contra uma API de mentira que imita a de verdade
(token que rotaciona, 500 mentiroso, captcha...). Uma sentinela reprova a suíte
se algum arquivo seu em `dados/` mudar. Se houver Chrome/Edge instalado, o login
pelo navegador é testado de verdade (headless, numa página local).

## Se der problema

* **Pede login toda hora**: outra coisa está logada na conta (celular, site, um
  servidor com o Tasker). A Lidl só aceita uma sessão. Use a pausa quando for
  usar a conta.
* **"Já está rodando"**: é o lock. O atalho só reabre a janela; pra encerrar, use
  *Sair* na tela ou crie `dados/tasker.stop`.
* **Refill não sai**: a aba *Log* filtrada por "refill" mostra o motivo (balde
  cheio, cooldown, espera depois de falhas, resposta da API).
* **Login pelo navegador não abre**: precisa de Chrome, Edge, Chromium ou Brave
  instalado. Sem nenhum, use número e senha.

## O que mudou da versão 1

A v1 mantinha um Chromium escondido (Playwright, ~300 MB) só pra fazer as chamadas
de dentro da página. Agora o Tasker fala HTTP direto com a API. Com isso foram
embora o watchdog de navegador travado, o keep-alive do portal, os cookies e o
relogin no Contao. Os ajustes que só existiam por causa do navegador (keep-alive,
headless, reabrir navegador) somem do `config.json` na próxima gravação; o resto
vale como estava. Monitor e tela agora são um processo só, e os ajustes valem sem
reiniciar. A versão cloud (`tasker_cloud.py`) virou o modo `--loop`/`--once` do
mesmo programa.
