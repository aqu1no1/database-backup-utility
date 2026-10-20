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

O teste atual `crates/database-adapters/tests/postgres_container.rs` comprova somente que
um PostgreSQL efêmero aceita `SELECT 1`. Ele **não** comprova que o motor de backup ou
restore já foi implementado.

## Regras

- Nunca executar testes destrutivos no banco ou volume de desenvolvimento.
- Evitar credenciais reais, fixtures contendo dados pessoais e dump de produção.
- Testes de cada adaptador devem cobrir sucesso, falhas controladas, integridade e restore real.
- Em CI, integração requer runner com Docker acessível.
- Frontend Angular deve testar i18n `pt-BR`/`en-US` e fallback quando existir.
