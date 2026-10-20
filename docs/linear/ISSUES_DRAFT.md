# Database Backup Utility — Proposta original de issues (arquivo histórico)

> **Status:** proposta original aprovada e cadastrada no Linear. As 12 issues oficiais são **DB-5 a DB-16**. Este arquivo é apenas histórico; consulte `docs/linear/README.md` e o Linear para o escopo e o status atuais.
>
> **Projeto Linear:** [Database Backup Utility](https://linear.app/db-backup-utility/project/database-backup-utility-575328110532)
>
> **Stack decidida:** Rust Workspace, CLI Rust, Angular + Tauri (desktop posterior), PNPM, PostgreSQL/MySQL/MongoDB via Docker em desenvolvimento, SQLite interno para histórico, Testcontainers para integração.
>
> **Publicações:** releases *somente* pelo botão **GitHub Actions > Release > Run workflow** (`workflow_dispatch`) com input de versão, a partir da branch `main`. Não publicar automaticamente em push de tag.

## Organização sugerida

- **Milestone v0.1.0 — PostgreSQL CLI MVP:** DBU-01 a DBU-10.
- **Milestone v0.2.0 — Multi-DB:** DBU-11 (separar por SGBD quando iniciar).
- **Milestone v0.3.0 — Desktop Angular/Tauri:** DBU-12 (detalhar em issues menores quando iniciar).
- **Labels propostas:** `area:rust`, `area:cli`, `area:database`, `area:infra`, `area:tests`, `area:security`, `area:docs`, `area:frontend`, `type:feature`, `type:chore`, `type:test`.

---

## DBU-01 — Inicializar monorepo, padrões de contribuição e pipelines

- **Tipo:** Chore / Infra
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:infra`, `area:rust`, `area:docs`, `type:chore`
- **Dependências:** nenhuma

### Descrição
Criar a estrutura inicial do `database-backup-utility` com Rust Workspace funcional, CLI mínima e infraestrutura de qualidade, contribuição e releases. Preparar o PNPM workspace para o futuro desktop Angular/Tauri, sem construir funcionalidades do frontend neste card.

### Escopo
- Inicializar `Cargo.toml` raiz e crates necessárias *agora* (sem diretórios vazios sem propósito), `apps/cli` e organização `crates/`, `infra/`, `tests/`, `docs/`.
- Adicionar `pnpm-workspace.yaml` e configuração de frontend quando efetivamente aplicável.
- Configurar `infra/docker/compose.yml` com PostgreSQL para desenvolvimento, `.env.example` e `.gitignore`.
- Adicionar `.github/pull_request_template.md`, `AGENTS.md`, `PROJECT.md`, `README.md` e `CHANGELOG.md` no formato Keep a Changelog.
- Criar `ci.yml`: `cargo fmt`, Clippy, build e testes unitários para PR/main; adicionar estrutura de integração com PostgreSQL quando o harness estiver pronto.
- Criar `release.yml` com gatilho **exclusivo** `workflow_dispatch`, input obrigatório `version`, seleção de `main`, verificação de versão/changelog e sem gatilho por tag.
- Criar `docs/CI.md` com convenções de branches, PRs, checks obrigatórios e releases manuais.

### Critérios de aceite
- [ ] `cargo check --workspace` e `cargo test --workspace` executam no workspace inicial.
- [ ] PostgreSQL de desenvolvimento sobe com Docker Compose sem instalar o servidor no host.
- [ ] PR aberto contra `main` dispara a CI Rust e o status é visível no GitHub.
- [ ] O template de PR contém seções específicas para Rust, Angular/Tauri, Docker e segurança.
- [ ] O workflow de release aparece com **Run workflow** e **não** possui `push.tags`; nenhuma publicação ocorre automaticamente.
- [ ] A documentação descreve como começar e testar o projeto.

### Fora do escopo
Implementar backup/restore, criar a UI Angular ou publicar uma release nesta issue.

---

## DBU-02 — Preparar testes de integração PostgreSQL com Testcontainers

- **Tipo:** Testes / Infra
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:tests`, `area:database`, `area:infra`
- **Dependências:** DBU-01

### Descrição
Implementar uma infraestrutura de testes isolados que inicia PostgreSQL efêmero via Testcontainers, aplica fixtures e encerra recursos ao término.

### Critérios de aceite
- [ ] Os testes não reutilizam nem apagam volumes do Compose de desenvolvimento.
- [ ] Porta e credenciais de teste são obtidas/configuradas sem hardcoding de segredos reais.
- [ ] Existem fixtures SQL simples e uma verificação de saúde do banco.
- [ ] O job de integração roda no CI Linux com Docker disponível e falha em caso de teste falho.
- [ ] Falhas apresentam informações úteis sem imprimir segredos.

---

## DBU-03 — Validar configurações e testar conexão PostgreSQL pela CLI

- **Tipo:** Feature
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:rust`, `area:cli`, `area:database`, `area:security`
- **Dependências:** DBU-01, DBU-02

### Descrição
Criar comando CLI para testar PostgreSQL com host, porta, usuário e database, solicitando segredos por mecanismo seguro quando necessário.

### Critérios de aceite
- [ ] Valida os parâmetros antes de conectar e utiliza timeouts.
- [ ] Distingue falha de autenticação, indisponibilidade e parâmetros inválidos quando possível.
- [ ] Não exibe senha em argumentos, logs, mensagens ou saída JSON.
- [ ] Testes cobrem conexão válida, senha inválida e banco indisponível.
- [ ] `--help` documenta o comando e os parâmetros.

---

## DBU-04 — Persistir histórico de backups em SQLite com migrações

- **Tipo:** Feature
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:rust`, `area:database`
- **Dependências:** DBU-01

### Descrição
Criar SQLite interno para armazenar metadados não sensíveis das conexões, execuções e artefatos, com migrações versionadas e localização documentada por sistema operacional.

### Critérios de aceite
- [ ] Migrations são aplicadas ao inicializar e testadas em banco vazio e já existente.
- [ ] Registros contêm timestamps, status, duração, localização e checksum quando aplicável.
- [ ] Senhas e segredos não são persistidos em texto puro.
- [ ] O banco fica em diretório de dados da aplicação, não no repositório.
- [ ] Há testes de leitura/gravação e evolução do esquema.

---

## DBU-05 — Implementar runner de ferramentas PostgreSQL (Docker e native)

- **Tipo:** Feature
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:rust`, `area:database`, `area:infra`, `area:security`
- **Dependências:** DBU-01, DBU-03

### Descrição
Criar interface para invocação controlada de `pg_dump`/`pg_restore`, com executor Docker para desenvolvimento e executor nativo quando os clientes estiverem instalados. Validar disponibilidade e compatibilidade das ferramentas.

### Critérios de aceite
- [ ] Ferramentas são invocadas por API de processo com argumentos estruturados, sem `sh -c`/`bash -c`.
- [ ] O runner detecta indisponibilidade, falha e cancelamento sem registrar segredos.
- [ ] O modo Docker usa uma imagem/cliente compatível e não depende de alterar o container de dados do usuário.
- [ ] Testes verificam ao menos sucesso e falha do runner.
- [ ] Diferenças entre Docker e native estão documentadas.

---

## DBU-06 — Executar backup completo PostgreSQL com streaming local

- **Tipo:** Feature
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:rust`, `area:cli`, `area:database`
- **Dependências:** DBU-03, DBU-04, DBU-05

### Descrição
Implementar backup full em formato adequado ao `pg_restore`, gravando o resultado em arquivo local sem manter todo o dump em memória; gerar manifesto/metadados e checksum.

### Critérios de aceite
- [ ] Backup de banco com fixtures gera arquivo válido e não vazio.
- [ ] Escrita usa streaming e arquivo temporário, evitando marcar um artefato parcial como concluído.
- [ ] Falhas são registradas no histórico SQLite com status e motivo sanitizado.
- [ ] O arquivo final contém checksum ou possui metadados para verificação.
- [ ] Formato e estratégia de compressão estão documentados (sem compressão dupla desnecessária).

---

## DBU-07 — Restaurar backup PostgreSQL em destino controlado

- **Tipo:** Feature
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:rust`, `area:cli`, `area:database`, `area:security`
- **Dependências:** DBU-05, DBU-06

### Descrição
Implementar operação de restore que verifica o arquivo, valida destino e exige confirmação explícita quando houver risco de sobrescrita.

### Critérios de aceite
- [ ] O restore funciona em banco temporário separado do banco de origem.
- [ ] Arquivo inexistente/corrompido é rejeitado com erro útil.
- [ ] Operação destrutiva exige confirmação explícita e possui proteções contra destino errado.
- [ ] O resultado do restore fica registrado no histórico.
- [ ] Há testes com dados restaurados e conferidos.

---

## DBU-08 — Padronizar UX da CLI e observabilidade das operações

- **Tipo:** Feature / Chore
- **Prioridade:** P1
- **Milestone:** v0.1.0
- **Labels:** `area:rust`, `area:cli`
- **Dependências:** DBU-03, DBU-04, DBU-06, DBU-07

### Descrição
Padronizar comandos e flags (`connection test`, `backup run`, `backup list`, `backup verify`, `restore`), mensagens, códigos de saída e logs com `tracing`.

### Critérios de aceite
- [ ] Cada comando tem `--help` útil e exemplos documentados.
- [ ] Erros retornam códigos de saída não zero e mensagens objetivas.
- [ ] Logs têm identificador de execução e não contêm segredos.
- [ ] Saída estruturada (`--json`) existe nos comandos em que for necessária para automação, sem duplicar regras de negócio.

---

## DBU-09 — Provar backup e restauração end-to-end com PostgreSQL temporário

- **Tipo:** Testes
- **Prioridade:** P0
- **Milestone:** v0.1.0
- **Labels:** `area:tests`, `area:database`, `area:cli`
- **Dependências:** DBU-02, DBU-06, DBU-07

### Descrição
Criar teste ponta a ponta: popular banco de origem, gerar backup pela CLI/core, restaurar em banco de destino descartável e comparar os dados essenciais.

### Critérios de aceite
- [ ] O teste roda localmente e no GitHub Actions com PostgreSQL isolado.
- [ ] A verificação compara esquema e registros relevantes, não apenas código de saída.
- [ ] Um teste de erro rejeita backup inválido/corrompido.
- [ ] O teste é determinístico e não precisa de intervenção humana.

---

## DBU-10 — Preparar documentação e release manual v0.1.0

- **Tipo:** Chore / Docs
- **Prioridade:** P1
- **Milestone:** v0.1.0
- **Labels:** `area:infra`, `area:docs`
- **Dependências:** DBU-03 a DBU-09

### Descrição
Fechar documentação pública, comandos de instalação, CHANGELOG e pipeline de release manual para Linux na v0.1.0.

### Critérios de aceite
- [ ] README explica pré-requisitos, setup Docker, backup e restore.
- [ ] `CHANGELOG.md` contém apenas entregas realizadas, seção `0.1.0` e links de comparação apropriados.
- [ ] Workflow **somente `workflow_dispatch`**, com input de versão e branch `main`, valida tag inexistente, versões Cargo e changelog.
- [ ] Pipeline executa testes/build antes de criar a tag Git.
- [ ] Release publica binário Linux da CLI, checksum SHA-256 e notas extraídas do changelog.
- [ ] Não há gatilho de release por `push` de tag.

---

## DBU-11 — Planejar e implementar adaptadores MySQL, MongoDB e SQLite

- **Tipo:** Feature / Épico futuro
- **Prioridade:** P2
- **Milestone:** v0.2.0
- **Labels:** `area:database`, `area:tests`
- **Dependências:** DBU-09

### Descrição
Evoluir o motor para os demais SGBDs. **Antes da execução, dividir este épico em issues separadas por banco**, com capacidades e limitações de backup/restore, formato, testes e runner.

### Critérios de aceite
- [ ] Cada SGBD possui contrato de capacidades documentado.
- [ ] Cada adaptador realiza backup full e restore em ambiente isolado.
- [ ] Não há promessa de incremental, diferencial ou PITR sem suporte confirmado.

---

## DBU-12 — Inicializar aplicativo desktop Angular + Tauri

- **Tipo:** Feature / Épico futuro
- **Prioridade:** P2
- **Milestone:** v0.3.0
- **Labels:** `area:frontend`, `area:rust`, `area:tests`
- **Dependências:** DBU-08

### Descrição
Preparar Angular standalone + TypeScript, Tailwind CSS, Tauri e integração segura com as mesmas crates usadas pela CLI. **Antes de iniciar, quebrar o épico em setup do desktop, conexões, backup, restore e histórico.**

### Critérios de aceite
- [ ] App inicia em modo desenvolvimento e compila para Linux.
- [ ] Comandos Tauri usam o motor Rust compartilhado; não duplicam a lógica no TypeScript.
- [ ] Testes Angular com Vitest e mocks de comandos Tauri passam.
- [ ] A CI Angular é habilitada quando os arquivos do app existirem.

---

## Regras propostas para uso no Linear

1. Registrar decisões arquiteturais em `PROJECT.md` ou ADRs. Linear contém tarefas, não substitui a documentação.
2. Ligar PRs a uma issue no Linear e manter os links cruzados.
3. Criar dependências somente onde elas afetam a ordem real de execução.
4. Não marcar cards como concluídos até os critérios de aceite e testes necessários passarem.
5. Atualizar `CHANGELOG.md` somente para mudanças efetivamente entregues; releases são sempre manuais pelo Run workflow.

**Aprovação pendente:** revisar títulos, quantidade de cards, milestones e escopo antes de criar qualquer issue no Linear.
