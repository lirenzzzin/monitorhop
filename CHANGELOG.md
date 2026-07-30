# Changelog

## 0.1.2 - 2026-07-30

- Faz o teclado seguir o `Begin` do cursor imediatamente, sem aguardar o ACK
  de entrada nem descartar teclas digitadas durante o round trip.
- Reduz o orçamento de software do fallback de retorno de 150 ms para 100 µs
  (0,1 ms), sem prometer tempo real do sistema operacional ou da rede.
- Usa limiar padrão de 1 px para liberar clientes X11 sem captura de borda.

## 0.1.1 - 2026-07-30

- Ativa por padrão o auto-release de 50 px para o cursor conseguir retornar de
  clientes X11 que não oferecem captura de borda.
- Corrige o CI do Windows para os runners com Visual Studio 2026.

## 0.1.0 - 2026-07-30

Primeira versão do MonitorHop.

### Adicionado

- Identidade própria de aplicação, configuração, IPC e protocolo.
- Clipboard de texto Unicode bidirecional até 2 MiB.
- Fragmentação do clipboard em datagramas MTU-safe.
- Remontagem tolerante a ordem e duplicação.
- Verificação de integridade SHA-256.
- ACK autenticado e retransmissão automática.
- Envio assíncrono para não bloquear mouse, teclado ou interface.
- Documentação específica para a topologia CachyOS/niri e
  EndeavourOS/X11.

### Base

- Lan Mouse 0.11.
- Mousehop 0.14.2.

O changelog da base imediata foi preservado em
`docs/UPSTREAM_MOUSEHOP_CHANGELOG.md`.
