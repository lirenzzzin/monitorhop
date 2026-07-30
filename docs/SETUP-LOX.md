# Configuração PC Lox + LoxNote

Topologia:

```text
[ LoxNote / EndeavourOS / X11 ]  [ pclox / CachyOS / niri ]
              esquerda                         principal
```

Endereços Tailscale observados em 2026-07-30:

- `pclox`: `100.67.175.56`
- `LoxNote`: `100.88.153.98`

## Instalação

Compile no PC:

```sh
cd /home/lorenzzo/Documentos/projetos/monitorhop
cargo build --release --workspace
install -Dm755 target/release/monitorhop ~/.local/bin/monitorhop
```

Copie o binário para o notebook:

```sh
scp target/release/monitorhop loxnote@100.88.153.98:/tmp/monitorhop
ssh loxnote@100.88.153.98 \
  'install -Dm755 /tmp/monitorhop ~/.local/bin/monitorhop'
```

Se o nome de usuário remoto não for `loxnote`, substitua-o nos dois comandos.

## Pares

No PC, adicione:

```toml
release_threshold_px = 50

[[clients]]
hostname = "LoxNote"
ips = ["100.88.153.98"]
position = "left"
activate_on_startup = true
clipboard_send = true
```

No notebook, adicione o PC como peer à direita:

```toml
[[clients]]
hostname = "pclox"
ips = ["100.67.175.56"]
position = "right"
activate_on_startup = true
clipboard_send = true
```

Depois de iniciar ambos, autorize as fingerprints pela interface e habilite
`clipboard_receive` nas duas conexões de entrada.

O `release_threshold_px` é necessário para o retorno suave do X11: ao empurrar
o cursor 50 px contra a borda direita do notebook, o PC libera a captura e
reposiciona o cursor na própria borda esquerda.

## Validação rápida

1. Copie no PC um texto Unicode com múltiplas linhas.
2. Cole no notebook.
3. Copie outro texto no notebook.
4. Cole no PC.
5. Copie um texto maior que 4 KiB para validar o protocolo fragmentado.
6. Atravesse o cursor pela borda esquerda do monitor principal.
