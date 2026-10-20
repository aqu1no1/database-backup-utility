# CI, Pull Requests e automações

## Branches e Pull Requests

- Branch principal `main` protegida; não commitar diretamente, exceto setup inicial se preciso.
- Padrão: `feat/*`, `fix/*`, `test/*`, `docs/*`, `chore/*`, `ci/*`.
- Abra PR vinculando a issue Linear e preencha `.github/pull_request_template.md`.
- Após todos os checks relevantes passarem, usar **Squash & Merge**.
- Não exigir revisão externa se o projeto tiver apenas um mantenedor, mas exigir checks na main.

## Workflows

| Workflow | Gatilho | Escopo |
| --- | --- | --- |
| `.github/workflows/ci.yml` | PR e push na main | Rustfmt, Clippy, testes unitários, build, integração PostgreSQL; Angular quando disponível |
| `.github/workflows/audit.yml` | Semanal + manual | Auditoria de dependências Rust |
| `.github/workflows/release.yml` | **Apenas `workflow_dispatch`** | Validação, testes, build Linux, tag e GitHub Release |

Configure em Settings → Rules → Rulesets a obrigatoriedade de PR e dos checks
`Rust / Quality & Unit` e `Rust / PostgreSQL Integration`. Confira os nomes reais
exibidos após a primeira execução. Angular só passa a ser exigido após ser implementado.

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
