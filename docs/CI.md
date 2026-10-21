# CI, Pull Requests e automações

## Branches e Pull Requests

- Branch principal `main` protegida; não commitar diretamente, exceto setup inicial se preciso.
- Sempre criar a branch a partir da `main` atualizada (`git switch main && git pull --ff-only`).
- Padrão `<tipo>/<id-linear>-<descrição>` em inglês, por exemplo `feat/db-9-pg-dump-runner`;
  tipos `feat`, `fix`, `docs`, `test`, `chore`, `ci`, `refactor`, `perf`.
- Commits e títulos de PR em inglês no padrão Conventional Commits. Detalhes na seção
  "Branches, commits e Pull Requests" do `AGENTS.md`.
- Abra PR vinculando a issue Linear e preencha `.github/pull_request_template.md`.
- Após todos os checks relevantes passarem, usar **Squash & Merge**.
- Não exigir revisão externa se o projeto tiver apenas um mantenedor, mas exigir checks na main.

## Workflows

| Workflow | Gatilho | Escopo |
| --- | --- | --- |
| `.github/workflows/ci.yml` | PR e push na main | Rustfmt, Clippy, testes unitários, build, integração PostgreSQL e E2E de backup/restore; Angular quando disponível |
| `.github/workflows/audit.yml` | Semanal + manual | Auditoria de dependências Rust |
| `.github/workflows/release.yml` | **Apenas `workflow_dispatch`** | Validação, testes, build Linux, tag e GitHub Release |

O job `Rust / PostgreSQL Integration` roda `cargo test --workspace --tests` com o cliente
PostgreSQL 17 instalado e `DBU_REQUIRE_NATIVE=1`, então os cenários E2E rodam com os runners
`docker` e `native`. Se o job falhar, o artefato **`e2e-logs`** (retenção de 7 dias) traz stdout e
stderr de cada passo da CLI nos cenários que falharam. Os testes conferem que nenhuma senha aparece
nessas saídas antes de gravá-las.

Configure em Settings → Rules → Rulesets a obrigatoriedade de PR e dos checks
`Rust / Quality & Unit` e `Rust / PostgreSQL Integration`. Confira os nomes reais
exibidos após a primeira execução. Angular só passa a ser exigido após ser implementado.

## Actions fixadas por SHA

Toda `uses:` aponta para um commit SHA, com a tag correspondente ao lado
(`actions/checkout@<sha> # v7.0.1`). Assim, uma tag movida ou comprometida no repositório da action
não muda o que roda aqui. Para atualizar:

```bash
gh api repos/actions/checkout/commits/v7 --jq .sha    # SHA atual da tag
```

Troque o SHA e a tag do comentário no mesmo commit. O `dtolnay/rust-toolchain` é fixado no SHA da
branch `stable` e recebe `toolchain: stable` explicitamente, porque fixado por SHA ele não deduz o
toolchain pelo nome da ref. Dependabot para `github-actions` é uma opção futura; não está configurado.

## Primeira configuração do repositório

1. Adicione estes arquivos em uma branch via PR.
2. Execute `cargo generate-lockfile` localmente, faça commit de `Cargo.lock`.
3. Confirme um PR com CI verde e só então configure required checks.
4. Não habilite disparos por push de tag em `.github/workflows/release.yml`.

## Segurança

- `pull_request`, **não** `pull_request_target`, para executar código de contribuidores.
- Use permissões `contents: read` nos jobs de CI.
- Somente o job de release possui permissão `contents: write`.
- Não disponibilizar secrets em PRs de forks; testes usam credenciais efêmeras locais.
- No setup do repositório, confirme que o `GITHUB_TOKEN` tem permissão de criar releases/tags
  no repositório, e que as regras de tags permitem escrita do GitHub Actions.

Para publicar releases, consulte `docs/RELEASE_PROCESS.md`.
