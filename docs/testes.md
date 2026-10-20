# Estratégia de testes

| Tipo | Ferramentas | Requer Docker |
| --- | --- | --- |
| Unitários Rust | `cargo test --workspace --lib --bins` | Não |
| Integração PostgreSQL | Testcontainers + tokio-postgres | Sim |
| E2E backup/restore (futuro) | CLI real, dois bancos temporários | Sim |
| Angular unit/component (futuro) | Vitest + Angular TestBed | Não |
| Desktop completo (futuro) | Ferramenta Tauri compatível | Conforme cenário |

## Comandos

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins
cargo test --workspace --tests
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

## Regras

- Nunca executar testes destrutivos no banco ou volume de desenvolvimento.
- Evitar credenciais reais, fixtures contendo dados pessoais e dump de produção.
- Testes de cada adaptador devem cobrir sucesso, falhas controladas, integridade e restore real.
- Em CI, integração requer runner com Docker acessível.
- Frontend Angular deve testar i18n `pt-BR`/`en-US` e fallback quando existir.
