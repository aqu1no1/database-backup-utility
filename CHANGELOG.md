# Changelog

Todas as mudanças relevantes deste projeto são registradas neste arquivo.

O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/) e o
versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

> Somente mudanças implementadas e testadas devem aparecer como entregues.
> As próximas versões estão planejadas em `docs/RELEASES.md`.

## [Unreleased]

### Added
- CLI `database-backup` com o comando `info`, que mostra o estado atual do projeto.
- Comando `test-connection` para validar parâmetros e testar a conexão PostgreSQL, com senha por prompt sem eco ou `DBU_PG_PASSWORD`, TLS verificado para hosts remotos e exit codes por tipo de falha.
- Comando `backup`: backup full PostgreSQL com `pg_dump` (formato custom) em streaming, executado pelo cliente instalado ou via Docker, com arquivo finalizado de forma atômica, SHA-256, arquivo de metadados ao lado do dump e registro no histórico SQLite.
- Comando `restore`: restaura um backup PostgreSQL após conferir SHA-256, estrutura e versão; cria um banco novo por padrão e só substitui um banco com dados com `--overwrite` e confirmação do nome, restaurando antes em um banco temporário e mantendo o anterior como `<destino>__previous_<data>`. O `pg_restore` roda em uma única transação e cada restore é registrado no histórico. Recusa symlinks e arquivos graváveis por grupo/outros como origem; `--allow-unverified` avisa e marca o restore como não verificado.

<!-- Ao concluir uma funcionalidade relevante, registre-a aqui usando:
### Added
- Mensagem da mudança validada.

Para publicar, mova as entradas para uma nova seção com data real:
Seção versionada: `[0.1.0] - AAAA-MM-DD`
O workflow manual rejeita releases sem seção publicada e sem mudanças reais. -->
