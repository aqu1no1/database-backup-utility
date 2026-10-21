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
## [X.Y.Z] - AAAA-MM-DD

### Added
- Mudança implementada e verificada.
```

O bloco acima é **apenas um exemplo**. Não registre funcionalidades ainda não implementadas.
O `[Unreleased]` fica vazio acima da nova seção, e o comentário de instruções fica dentro dele:
o script extrai o texto da seção da versão até o fim do arquivo, então nada além das notas
pode vir depois dela.

5. Validar localmente com os mesmos passos do job de release:

```bash
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
python3 .github/scripts/release_notes.py --version vX.Y.Z      # confere versão de todos os membros do workspace e o changelog
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --lib --bins --locked
cargo test --workspace --tests --locked                         # Docker necessário
cargo build --locked --release -p database-backup-cli --bin database-backup
./target/release/database-backup --version                      # deve mostrar X.Y.Z
```

6. Enviar via PR e fazer merge na `main` quando os checks passarem.

## Publicação (somente manual)

1. Acesse `Actions > Release > Run workflow` no GitHub.
2. Selecione `main` e informe, por exemplo, `v0.1.0`.
3. A pipeline valida a versão do workspace e a seção de changelog, ausência de tag,
   formatação, Clippy, testes unitários, integração PostgreSQL e build da CLI.
4. Após tudo passar, cria a **tag anotada na SHA validada**, envia ao repositório e
   publica um GitHub Release com as notas da versão, tarball Linux x86_64 e SHA256SUMS.

## Depois de publicar

1. Baixar o tarball e o `SHA256SUMS.txt` do Release e rodar `sha256sum -c SHA256SUMS.txt`.
2. Extrair e conferir `database-backup --version`.
3. Mover o card de release para Done no Linear e fechar o milestone.

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
