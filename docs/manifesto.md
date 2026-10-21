# Manifesto do backup

Cada backup gerado por `backup run` tem, ao lado do dump, um manifesto JSON versionado:
`<arquivo>.dump.meta.json`. Com ele dá para identificar e verificar o backup sem o histórico
SQLite interno, por exemplo depois de copiar o par de arquivos para outra máquina.

## Quando e como é gravado

- Só depois que o dump terminou, o `pg_dump` saiu com sucesso, o arquivo passou no
  `pg_restore --list` e foi renomeado do `.partial` para o nome final.
- Escrita atômica: arquivo `.partial`, `fsync` e `rename`, com permissão `0600`.
- Se o manifesto não puder ser gravado, o dump também é removido e o backup falha.

## Campos (versão 1)

```json
{
  "manifest_version": 1,
  "tool": { "name": "database-backup", "version": "0.1.0" },
  "engine": "postgresql",
  "format": "pg_dump-custom",
  "file_name": "app-20261021T120000Z.dump",
  "sha256": "…64 caracteres hex…",
  "size_bytes": 5302,
  "database": "app",
  "host": "localhost",
  "port": 5432,
  "server_version": "17.2 (Debian 17.2-1.pgdg120+1)",
  "pg_dump_version": "17.11",
  "runner": "docker",
  "created_at": "2026-10-21T12:00:00Z",
  "execution_id": "01a15ec9-…"
}
```

| Campo | Significado |
| --- | --- |
| `manifest_version` | Versão do formato do manifesto. |
| `tool` | Nome e versão do utilitário que gerou o backup. |
| `engine` | SGBD de origem. Hoje sempre `postgresql`. |
| `format` | Formato do dump (`pg_dump-custom`). |
| `file_name` | Nome do dump, sem diretório. Mover o par junto continua válido. |
| `sha256`, `size_bytes` | Checksum e tamanho do dump. |
| `database`, `host`, `port` | Origem do backup. Nunca usuário, senha, `secret_ref` ou URI de conexão. |
| `server_version`, `pg_dump_version`, `runner` | Versões usadas e se o `pg_dump` foi nativo ou via Docker. |
| `created_at`, `execution_id` | Momento do backup e ID da execução no histórico de origem. |

## Versões

- `manifest_version` só muda quando o formato muda de forma incompatível (campo removido,
  renomeado ou com outro significado). Campos novos opcionais não mudam a versão.
- Leitores recusam manifesto sem `manifest_version` ou com versão desconhecida, com código 10 e a
  versão encontrada na mensagem. Assim, um utilitário antigo nunca interpreta errado um formato novo.

## O que `backup verify` e `restore run` conferem

1. O dump e o manifesto não são links simbólicos nem graváveis por grupo ou outros.
2. `manifest_version` suportada.
3. `file_name` igual ao nome do dump verificado: um manifesto copiado ou renomeado para outro dump é recusado.
4. `engine` igual a `postgresql`.
5. `size_bytes` igual ao tamanho real (detecta arquivo truncado antes do hash).
6. SHA-256 do dump igual ao `sha256` do manifesto.
7. Estrutura válida com `pg_restore --list`.

Sem manifesto, a referência do checksum vem do histórico SQLite. Sem nenhum dos dois, só com
`--allow-unverified`, e o restore fica registrado como não verificado.

## Limites

O SHA-256 detecta corrupção e alterações acidentais ou parciais, mas **não autentica** o backup.
Quem consegue alterar o dump e o manifesto ao mesmo tempo produz um par consistente, que passa na
verificação. Proteger contra isso exige guardar o checksum em outro lugar confiável ou assinar ou
criptografar o artefato. A criptografia com `age` está planejada no DB-31 (v0.4.0). Até lá, proteja o
diretório de backups com as permissões do sistema e, para cópias externas, guarde o `sha256` em
um local separado.
