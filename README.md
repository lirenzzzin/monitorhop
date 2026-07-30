# MonitorHop

MonitorHop é um KVM de software para compartilhar mouse, teclado e clipboard
entre computadores. Ele é um fork GPL do
[lan-mouse](https://github.com/feschber/lan-mouse), incorpora melhorias do
[Mousehop](https://github.com/jondkinney/mousehop) e adiciona um protocolo de
clipboard confiável para uso diário.

O objetivo principal é funcionar bem em combinações Linux reais, inclusive um
servidor Wayland/niri controlando um cliente X11.

Repositório: [lirenzzzin/monitorhop](https://github.com/lirenzzzin/monitorhop)

## O que funciona

- Mouse e teclado com troca pela borda da tela.
- Clipboard de texto Unicode bidirecional, inclusive texto multilinha e código.
- Conteúdo de clipboard de até 2 MiB.
- Fragmentação em datagramas de até 1200 bytes, evitando depender de
  fragmentação IP.
- Verificação SHA-256 após remontagem.
- Confirmação do receptor e até quatro retransmissões automáticas.
- Remontagem de blocos duplicados ou fora de ordem.
- Supressão de eco e de loops quando existem mais de duas máquinas.
- Autorização por fingerprint e transporte DTLS criptografado.
- Permissões independentes por par: enviar e receber clipboard.
- Lista de aplicativos cujo clipboard nunca deve ser enviado, útil para
  gerenciadores de senha.
- Interface GTK/libadwaita e modo daemon.

O clipboard da versão 0.1 sincroniza **texto**. Imagens, arquivos e tipos MIME
arbitrários ainda não fazem parte do protocolo.

## Compilar no Arch, CachyOS ou EndeavourOS

Dependências do sistema:

```sh
sudo pacman -S --needed base-devel gtk4 libadwaita libx11 libxtst
```

O projeto fixa o Rust 1.95 em `rust-toolchain.toml`. Com `rustup` disponível:

```sh
cargo build --release --workspace
```

O executável fica em:

```text
target/release/monitorhop
```

Para instalar apenas para o usuário:

```sh
install -Dm755 target/release/monitorhop ~/.local/bin/monitorhop
```

Na primeira execução, a aplicação instala o arquivo `.desktop` e o ícone no
perfil do usuário.

## Configurar duas máquinas

Execute `monitorhop` nas duas máquinas. Na interface:

1. Adicione a outra máquina como peer e informe hostname ou IP.
2. Escolha a posição física do peer.
3. Autorize a fingerprint apresentada pela outra máquina.
4. Ative `Send clipboard` no peer de saída.
5. Ative `Receive clipboard` na fingerprint de entrada.

As duas permissões precisam estar ativas para cada direção desejada. Isso evita
que uma atualização habilite transmissão de clipboard sem consentimento.

A configuração fica em:

```text
~/.config/monitorhop/config.toml
```

Exemplo reduzido:

```toml
port = 4252
release_threshold_px = 50

[authorized_fingerprints."fingerprint-do-peer"]
description = "notebook"
clipboard_receive = true

[[clients]]
hostname = "notebook"
ips = ["100.64.0.2"]
position = "left"
activate_on_startup = true
clipboard_send = true
```

`release_threshold_px = 50` devolve o cursor ao computador de origem quando
você o empurra contra a borda de retorno. O valor é o padrão do MonitorHop e é
especialmente importante quando o destino usa X11, que nesta versão recebe
mouse e teclado, mas não possui captura de borda própria. Use `0` para
desativar e depender apenas do atalho de liberação ou do evento enviado pelo
peer.

## Como o clipboard confiável funciona

Cada alteração local recebe um identificador de transferência. O texto é
codificado como UTF-8, limitado a 2 MiB e dividido em blocos de 1120 bytes.
O início da transferência declara tamanho, quantidade de blocos e SHA-256.

O receptor aceita blocos duplicados ou fora de ordem, remonta o texto, verifica
tamanho e hash e só então o encaminha ao clipboard local. Ele responde com um
ACK autenticado pelo mesmo canal DTLS. Se o ACK não chegar, o emissor repete a
transferência até quatro vezes.

O envio ocorre em tarefas separadas, então uma transferência grande não
interrompe mouse, teclado nem a interface.

## Privacidade e segurança

- Clipboard é desativado por padrão.
- O emissor e o receptor precisam permitir explicitamente cada direção.
- Peers são autenticados por fingerprint de certificado.
- Todo tráfego utiliza DTLS.
- Aplicativos sensíveis podem ser colocados na lista de supressão.
- No macOS, pasteboards marcados como conteúdo oculto são ignorados
  automaticamente.

Nenhum conteúdo passa por nuvem ou serviço externo.

## Desenvolvimento

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

O protocolo e os limites públicos estão em
`monitorhop-proto/src/lib.rs`. A captura do clipboard fica em
`input-capture/src/clipboard.rs`; envio confiável e ACK ficam em
`monitorhop/src/connect.rs` e `monitorhop/src/listen.rs`.

## Licença e créditos

GPL-3.0-or-later. Consulte `LICENSE` e `NOTICE`.

- Lan Mouse: Ferdinand Schober e contribuidores.
- Mousehop: Jon Kinney e contribuidores.
- MonitorHop: contribuidores do MonitorHop.
