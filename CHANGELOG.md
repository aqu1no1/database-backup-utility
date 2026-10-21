# Changelog

Todas as mudanças relevantes deste projeto são registradas neste arquivo.

O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/) e o
versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

> Somente mudanças implementadas e testadas devem aparecer como entregues.
> As próximas versões estão planejadas em `docs/RELEASES.md`.

## [Unreleased]

### Added
- `THIRD-PARTY-LICENSES.md` no tarball da release, gerado com `cargo-about`, com os avisos de licença das dependências embutidas no binário.

### Security
- `unsafe` proibido em todo o workspace.
- Dependências verificadas por `cargo-deny` em cada PR (advisories RustSec, licenças, fontes e wildcards), com triagem documentada em `docs/seguranca-rust.md`.
- Toolchain do Rust fixada em `rust-toolchain.toml` e Cargo com `--locked` em toda a CI.

<!-- Ao concluir uma funcionalidade relevante, registre-a aqui usando:
### Added
- Mensagem da mudança validada.

Para publicar, mova as entradas para uma nova seção com data real:
Seção versionada: `[X.Y.Z] - AAAA-MM-DD`
O workflow manual rejeita releases sem seção publicada e sem mudanças reais. -->

## [0.1.0] - 2026-10-21

Primeira versão: CLI de backup e restauração de PostgreSQL para Linux x86_64. Requer o cliente
PostgreSQL (`pg_dump`/`pg_restore`) com versão maior ou igual à do servidor, ou Docker.

### Added
- CLI `database-backup` organizada em `connection test`, `backup run|list|verify` e `restore run|list`, com exemplos em cada `--help` e exit codes documentados por tipo de falha.
- `connection test`: valida parâmetros e testa a conexão PostgreSQL, com TLS e certificado verificados para hosts remotos.
- `backup run`: backup full com `pg_dump` (formato custom) em streaming, pelo cliente instalado ou via Docker. O arquivo é finalizado de forma atômica (`.partial` + `rename`), com permissão `0600`, SHA-256, manifesto ao lado e registro no histórico SQLite. Ctrl+C ou timeout não deixam arquivo parcial nem container do runner. O comando avisa quando o diretório de saída é acessível por outros usuários.
- Manifesto versionado `<dump>.meta.json` (`manifest_version` 1): SGBD, formato, nome do arquivo, versão do utilitário, tamanho e SHA-256, sem credenciais.
- `backup verify`: confere manifesto, tamanho, SHA-256 e estrutura (`pg_restore --list`) sem restaurar e sem depender do histórico.
- `restore run`: restaura depois de conferir origem, manifesto, checksum, estrutura e versão. Cria um banco novo por padrão e só substitui um banco com dados com `--overwrite` e confirmação do nome. Nesse caso restaura antes em um banco temporário e mantém o anterior como `<destino>__previous_<data>`. O `pg_restore` roda em uma única transação. Arquivos sem checksum conhecido exigem `--allow-unverified`, e o restore fica marcado como não verificado.
- `backup list` e `restore list` consultam o histórico interno.
- Senha por prompt sem eco, `DBU_PG_PASSWORD` ou arquivo (`--password-file`/`DBU_PG_PASSWORD_FILE`, só com permissão `600`/`400`). Mais de uma fonte definida é erro. O histórico registra só a origem da senha, nunca o valor.
- Opção global `--json` com o resultado (ou erro e exit code) em uma linha JSON no stdout.
- Logs estruturados com `tracing` no stderr (horário, operação, IDs, status e duração, sem credenciais), configuráveis por `--log-level` ou `RUST_LOG`.
- Licença dupla **MIT OR Apache-2.0** (`LICENSE-MIT`, `LICENSE-APACHE`), incluída no tarball da release junto com o README.

### Security
- Senhas nunca vão em argumentos de processo: os utilitários recebem um `.pgpass` temporário e privado.
- O nome do banco é passado aos utilitários sempre como valor entre aspas (`dbname='...'`), para que nomes contendo `=` não sejam interpretados como string de conexão.
- Origens de restore recusadas: links simbólicos e arquivos (ou manifestos) graváveis por grupo ou outros.
- GitHub Actions fixadas por commit SHA; Clippy proíbe `unwrap()`/`expect()` fora de testes.
- Controles, evidências e riscos aceitos documentados em `docs/seguranca.md`.
