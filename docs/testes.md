# Estratégia de testes

| Tipo | Ferramentas | Requer Docker |
| --- | --- | --- |
| Unitários Rust | `cargo test --workspace --lib --bins` | Não |
| Integração PostgreSQL | Testcontainers + tokio-postgres | Sim |
| E2E backup/restore | CLI real, dois servidores PostgreSQL temporários | Sim |
| Angular unit/component (futuro) | Vitest + Angular TestBed | Não |
| Desktop completo (futuro) | Ferramenta Tauri compatível | Conforme cenário |

## Comandos

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins
cargo test --workspace --tests
cargo test -p database-backup-cli --test e2e
cargo build --workspace
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```

## Integração PostgreSQL

O harness `PostgresTestDb` (`crates/database-adapters/tests/support/mod.rs`) sobe um
`postgres:17` efêmero via Testcontainers e entrega um client `tokio-postgres` conectado:

- `PostgresTestDb::start()` cria um banco vazio; `start_seeded()` também aplica
  `tests/fixtures/postgres/seed.sql`.
- A porta é escolhida pelo Docker e a senha é gerada a cada execução; nada é lido do `.env`
  nem do Compose de desenvolvimento.
- O container é removido quando o valor sai de escopo, inclusive se o teste falhar.
- Erros de inicialização e conexão informam imagem, host e porta, nunca a senha.

Para usar em um novo arquivo de teste do crate:

```rust
mod support;

use support::{PostgresTestDb, TestResult};

#[tokio::test]
async fn exemplo() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let total: i64 = db.client().query_one("SELECT COUNT(*) FROM customers", &[]).await?.get(0);
    assert_eq!(total, 3);
    Ok(())
}
```

Os testes atuais (`crates/database-adapters/tests/postgres_container.rs`) cobrem saúde do
banco, aplicação das fixtures e isolamento entre dois bancos de teste. Eles **não** comprovam
que o motor de backup ou restore já foi implementado.

## E2E de backup e restauração

`apps/cli/tests/e2e.rs` executa o binário real da CLI contra servidores PostgreSQL efêmeros. A
origem recebe `tests/fixtures/postgres/e2e.sql`:
- dois schemas, FK, `UNIQUE`, `CHECK`, índice, sequence, view e função;
- tipos `jsonb`, `bytea`, `numeric`, `timestamptz` e arrays;
- texto com acentos/emoji e algumas centenas de linhas.

O destino é **outro container**.

| Cenário | O que comprova |
| --- | --- |
| `full_cycle_restores_identical_data_on_another_server` | `backup run` → `backup verify` → `restore run` deixa o destino idêntico à origem. Também confere permissões `0700`/`0600`, `backup list`/`restore list` e que nenhum segredo foi gravado no histórico ou nos `.meta.json`. Roda com cada runner disponível. |
| `connection_failures_have_distinct_exit_codes` | Servidor inacessível (5), senha errada (3) e banco inexistente (4), em `backup run` e `restore run`. |
| `corrupted_and_partial_archives_are_rejected` | Checksum divergente (10), arquivo truncado, `.partial` e destino ausente (2), sem criar bancos no destino. |
| `interrupted_operations_leave_no_partial_results` | Ctrl+C (SIGINT) no meio do dump: sai com 9, não sobra `.dump` nem `.partial` e o histórico fica `canceled`. |
| `destructive_restore_requires_confirmation` | Sem `--overwrite` ou com `--confirm` errado sai com 12. Com confirmação, o banco anterior é mantido em `__previous_*`. |
| `verify_and_restore_without_history` | O dump e o manifesto copiados para um ambiente com histórico vazio são verificados e restaurados idênticos. Dump alterado, `sha256` adulterado no manifesto e dump ausente saem com 10. |
| `hostile_identifiers_are_not_executed` | Nomes com `;`, `$(...)`, crases, aspas, `dbname=...` e prefixo `-` são tratados literalmente ou recusados; nada é executado nem escrito em outro banco. |

Como funciona:

- **Comparação por snapshot:** schema (colunas, constraints, índices, views, funções, sequences) e,
  por tabela, contagem + `md5` das linhas ordenadas. O snapshot não guarda dados brutos.
- **Segredos:** toda execução da CLI passa por um helper que falha o teste se a senha aparecer no
  stdout/stderr, inclusive com `--log-level debug`.
- **Resíduos:** ao fim de cada cenário, nenhum container `dbu-pg-*`, nenhum temporário no `TMPDIR`
  próprio do teste e nenhum `.partial`. Os cenários rodam em série dentro do arquivo, para essa
  checagem não ver containers de outro cenário.
- **Runners:** `docker` sempre; `native` quando `pg_dump` está instalado. Com `DBU_REQUIRE_NATIVE=1`
  (na CI), o runner nativo é obrigatório.
- **Diagnóstico:** se um cenário falhar, stdout/stderr de cada passo vão para
  `target/e2e-logs/<cenário>/` ou para `DBU_E2E_LOG_DIR`.
- **Timeout:** cada cenário tem limite de 300s.

## Regras

- Nunca executar testes destrutivos no banco ou volume de desenvolvimento.
- Evitar credenciais reais, fixtures contendo dados pessoais e dump de produção.
- Testes de cada adaptador devem cobrir sucesso, falhas controladas, integridade e restore real.
- Em CI, integração requer runner com Docker acessível.
- Frontend Angular deve testar i18n `pt-BR`/`en-US` e fallback quando existir.
