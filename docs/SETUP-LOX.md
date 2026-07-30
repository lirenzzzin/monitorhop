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
release_threshold_px = 1

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

O `release_threshold_px` é necessário para o retorno do X11. Com o valor `1`,
o primeiro pixel de movimento além da borda direita modelada do notebook
agenda a liberação no PC em 100 µs (0,1 ms), sem aguardar um round trip. Esse
é um orçamento de software, não uma garantia de tempo real do sistema
operacional. O teclado passa a seguir o mesmo `Begin` do cursor sem esperar
ACK.

## Validação rápida

1. Copie no PC um texto Unicode com múltiplas linhas.
2. Cole no notebook.
3. Copie outro texto no notebook.
4. Cole no PC.
5. Copie um texto maior que 4 KiB para validar o protocolo fragmentado.
6. Atravesse o cursor pela borda esquerda do monitor principal.
