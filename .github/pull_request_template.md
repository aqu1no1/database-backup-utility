# Pull Request

## 📌 Descrição

<!-- Resuma o que foi alterado e qual resultado se espera desta PR. -->

## 🏷 Tipo de PR

<!-- Selecione o tipo predominante. -->

- [ ] 🐛 Bugfix
- [ ] ✨ Feature
- [ ] ♻️ Refactor
- [ ] ⚡ Performance
- [ ] 🧪 Testes
- [ ] 📚 Documentação
- [ ] 🔧 Chore
- [ ] 🚨 Hotfix
- [ ] 🔄 CI/CD

## 🧩 Áreas afetadas

<!-- Marque todas as áreas modificadas. -->

- [ ] Rust Core / crates compartilhadas
- [ ] CLI Rust (`apps/cli`)
- [ ] Adaptadores de banco / backup / restore
- [ ] Armazenamento, agendamento ou histórico SQLite
- [ ] Angular (`apps/desktop/src`)
- [ ] Tauri (`apps/desktop/src-tauri`)
- [ ] Docker, testes ou infraestrutura
- [ ] GitHub Actions / releases
- [ ] Documentação

## 🔗 Links relacionados

- Linear: <!-- link da issue -->
- GitHub Issue/PR: <!-- se houver -->

## 🧠 Contexto e abordagem

<!-- Qual problema é resolvido? Por que esta solução foi escolhida? Houve trade-offs relevantes? -->

## 🧪 Como validar

<!-- Inclua passos reproduzíveis, comandos relevantes e resultado esperado. -->

## 📋 Checklist geral

- [ ] A mudança está limitada ao escopo da issue, sem refatorações não solicitadas.
- [ ] O código segue as convenções existentes e remove artefatos que esta PR tornou desnecessários.
- [ ] Não há logs de debug, código comentado ou dependências desnecessárias.
- [ ] Testes foram incluídos ou atualizados para comportamentos não triviais; quando não aplicável, expliquei o motivo.
- [ ] Documentação foi atualizada quando necessário (`README.md`, `PROJECT.md`, `AGENTS.md` ou `docs/`).
- [ ] `CHANGELOG.md` foi atualizado em `[Unreleased]` se houver mudança relevante para usuários.

### 🦀 Rust — preencher somente se aplicável

- [ ] `cargo fmt --all -- --check` passou.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passou.
- [ ] `cargo test --workspace` passou (ou informei o motivo de qualquer teste não executado).
- [ ] Erros recuperáveis usam `Result` e não dependem de `unwrap()` em caminhos de produção.
- [ ] Operações de backup/restore evitam carregar arquivos grandes inteiramente em memória.
- [ ] Comandos externos são executados com argumentos estruturados, sem interpolação em shell.

### 🅰️ Angular / Tauri — preencher somente se aplicável

- [ ] Componentes Angular seguem o padrão standalone e tipagem estrita do projeto.
- [ ] Estado e serviços seguem os padrões existentes (Signals / RxJS quando apropriado).
- [ ] `pnpm --dir apps/desktop lint` passou (quando o script existir).
- [ ] `pnpm --dir apps/desktop test` passou (quando o script existir).
- [ ] `pnpm --dir apps/desktop build` passou (quando o aplicativo existir).
- [ ] Comandos Tauri validam dados recebidos e usam permissões mínimas.
- [ ] Novos textos foram internacionalizados se o recurso de i18n já estiver configurado.

### 🐳 Docker, banco de dados e testes — preencher somente se aplicável

- [ ] Os testes de integração utilizam bancos isolados/temporários (Testcontainers ou infraestrutura de testes dedicada).
- [ ] Testes cobrem o cenário de falha relevante, quando aplicável.
- [ ] Mudanças no esquema do histórico SQLite incluem migration versionada.
- [ ] Backups/restore foram verificados em um banco de destino descartável, sem alterar dados de produção.
- [ ] Não há containers, volumes ou arquivos de teste residuais criados pela mudança.

### 🔐 Segurança — preencher quando aplicável

- [ ] Não foram incluídos segredos, senhas, tokens, dados pessoais ou dumps reais nos commits, logs ou evidências.
- [ ] Credenciais não são passadas como argumentos visíveis na lista de processos quando existir alternativa segura.
- [ ] Novas configurações sensíveis usam o provedor de segredos definido pelo projeto.
- [ ] A integridade dos arquivos de backup e os riscos de sobrescrita no restore foram considerados.

### ⚙️ Variáveis de ambiente — somente se alteradas

- [ ] `.env.example` e arquivos de exemplo relevantes foram atualizados, sem valores sensíveis.
- [ ] Configuração, validação e documentação das novas variáveis foram atualizadas.
- [ ] Docker/CI receberam apenas as variáveis necessárias, com secrets quando apropriado.

#### Variáveis adicionadas (sem valores sensíveis)

```dotenv
# NOME_DA_VARIAVEL=
```

#### Variáveis removidas

```dotenv
# NOME_DA_VARIAVEL
```

## 📸 Evidências (se aplicável)

<!-- Saída resumida de testes, logs sem dados sensíveis, screenshots Angular/Tauri ou demonstração da CLI. -->

## 🚀 Observações adicionais

<!-- Limitações, migrações necessárias, compatibilidade de SO, riscos conhecidos ou passos pós-merge. -->

<!-- Releases do projeto são exclusivamente manuais pelo GitHub Actions > Release > Run workflow, na branch main, via workflow_dispatch. Esta PR não deve introduzir publicação automática por push de tag. -->
