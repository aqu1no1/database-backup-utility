# Processo de release manual

**Decisão invariável:** releases são publicadas **somente** em GitHub → Actions → Release →
**Run workflow**, selecionando a branch `main` e a versão `vX.Y.Z`.
Não há push de tag como gatilho e merges na `main` não publicam automaticamente.

## Preparar a release (via Pull Request)

1. Concluir e validar os requisitos da milestone no Linear.
2. Atualizar `[workspace.package].version` em `Cargo.toml` e os manifests que
   herdam essa versão.
3. Garantir que `Cargo.lock` esteja versionado (`cargo generate-lockfile`, se necessário).
4. Transferir entradas implementadas e verificadas de `[Unreleased]` para uma seção como:

```markdown
## [0.1.0] - 2026-10-20

### Added
- Comando de backup PostgreSQL validado por testes E2E.
```

O bloco acima é **apenas um exemplo**. Não registre funcionalidades ainda não implementadas.

5. Validar com:

```bash
python3 .github/scripts/release_notes.py --version v0.1.0
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins
cargo test --workspace --tests
cargo build --release -p database-backup-cli --bin database-backup
```

6. Enviar via PR e fazer merge na `main` quando os checks passarem.

## Publicação (somente manual)

1. Acesse `Actions > Release > Run workflow` no GitHub.
2. Selecione `main` e informe, por exemplo, `v0.1.0`.
3. A pipeline valida a versão do workspace e a seção de changelog, ausência de tag,
   formatação, Clippy, testes unitários, integração PostgreSQL e build da CLI.
4. Após tudo passar, cria a **tag anotada na SHA validada**, envia ao repositório e
   publica um GitHub Release com as notas da versão, tarball Linux x86_64 e SHA256SUMS.

## Observações de segurança

- A primeira publicação só deve ocorrer quando a CLI realmente realizar backup/restore.
- Não inclua `.env`, senhas, dumps, logs privados nem arquivos temporários nos artefatos.
- O pipeline exige execução da `main`; não execute a partir de uma tag ou branch alternativa.
- Se o upload da release falhar **depois** de criar a tag, a tag já existirá; não force-push
  nem reescreva tags. Investigue e faça recuperação manual controlada.
- Se regras de branch/tag impedirem criação de tags pelo GitHub Actions, configure
  explicitamente a permissão exigida no repositório.
- Futuramente o pipeline será ampliado para os instaladores Tauri/Angular, Windows e macOS,
  somente depois que esses artefatos existirem e forem testados.
