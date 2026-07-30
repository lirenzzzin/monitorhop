# Nix

Execute o flake diretamente a partir do checkout:

```sh
nix run .
nix run . -- --help
```

Ou diretamente do GitHub:

```sh
nix run github:lirenzzzin/monitorhop
```

Como entrada de outro flake:

```nix
{
  inputs.monitorhop.url = "github:lirenzzzin/monitorhop";
}
```
