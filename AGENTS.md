# AGENTS.md — Database Backup Utility

Este arquivo define as instruções de trabalho para agentes de IA que contribuam com o
monorepo `database-backup-utility`. Siga estas orientações em conjunto com o código-fonte,
o `README.md` e a documentação relevante. O código existente é a fonte de verdade:
não presuma que módulos ou dependências planejados já foram implementados.

## 1. Visão geral do projeto

O projeto é uma ferramenta multiplataforma para backup e restauração de bancos de dados,
com duas interfaces que compartilham a mesma implementação em Rust:

- **CLI:** aplicação de linha de comando em Rust para operações manuais e automatizadas.
- **Desktop (planejado para a v0.3.0):** aplicação Tauri com **Angular standalone + TypeScript**,
  **Tailwind CSS v4** e **Transloco para i18n** (`pt-BR`, `en-US`). React não faz parte deste projeto.
- **Crates Rust compartilhadas:** lógica de backup, restauração, adaptadores de banco,
  armazenamento e agendamento.
- **SGBDs previstos:** PostgreSQL, MySQL, MongoDB e SQLite.
- **Infraestrutura:** Docker Compose para desenvolvimento local com bancos de dados;
  Testcontainers para testes de integração isolados.
- **Releases:** versionamento semântico, `CHANGELOG.md` e GitHub Releases
  **exclusivamente pelo acionamento manual `workflow_dispatch` / Run workflow no GitHub Actions**.
  A CI continua automática para Pull Requests e para a branch `main`.

A primeira milestone de implementação é um fluxo funcional de CLI para PostgreSQL:
validação da conexão, backup completo, restauração em destino isolado, verificação e testes.
Angular/Tauri, bancos adicionais, provedores de nuvem e modos avançados de backup são
etapas futuras, **não funcionalidades já disponíveis**.

## 2. Princípios de trabalho

### Pense antes de programar

- Inspecione primeiro os arquivos relevantes, os testes próximos e os padrões existentes.
- Para tarefas não triviais, apresente brevemente o objetivo, as premissas importantes e os
  critérios de sucesso.
- Quando uma ambiguidade afetar segurança, risco de perda de dados, contratos de API ou
  arquitetura, peça esclarecimento antes de prosseguir. Nos demais casos, adote uma
  alternativa conservadora e explicite a premissa.
- Apresente os trade-offs quando forem relevantes para a alteração proposta.
- Prefira uma solução simples a uma estrutura extensível criada por especulação.

### Mantenha as alterações pequenas e focadas

- Implemente somente o comportamento solicitado e os testes/documentação necessários para
  validá-lo.
- Não refatore código adjacente, renomeie símbolos sem relação com a tarefa ou atualize
  dependências sem um motivo concreto.
- Reutilize utilitários e convenções existentes antes de introduzir abstrações.
- Remova imports, variáveis e arquivos temporários que se tornaram desnecessários **em razão
  das suas próprias alterações**. Informe dívidas técnicas não relacionadas, sem modificá-las
  silenciosamente.
- Evite abstrações de uso único, a menos que uma fronteira arquitetural clara as justifique.
- Priorize clareza e correção, não soluções engenhosas ou otimização prematura.

### Verifique o resultado

- Defina critérios de sucesso observáveis: testes, saída esperada e caminhos de erro.
- Ao corrigir um bug, reproduza o problema quando viável e adicione um teste de regressão.
- Execute primeiro as verificações mais específicas e depois, quando disponíveis, as mais amplas.
- Nunca afirme que um comando, teste, backup ou restore funcionou sem tê-lo executado e
  obtido evidências.
- Se uma ferramenta ou ambiente estiver indisponível, explique exatamente o que não pôde
  ser verificado e por quê. Não invente resultados de testes.

## 3. Limites e responsabilidades no monorepo

Estrutura prevista; crie diretórios somente quando forem necessários à tarefa:

```text
database-backup-utility/
├── AGENTS.md
├── apps/
│   ├── cli/                      # Executável Rust (clap)
│   └── desktop/
│       ├── src/                  # Aplicação Angular
│       ├── src-tauri/            # Comandos Tauri e integração Rust
│       └── package.json
├── crates/
│   ├── core/                     # Tipos do domínio, casos de uso e portas
│   ├── database-adapters/        # PostgreSQL, MySQL, MongoDB e SQLite
│   ├── storage/                  # Adaptadores locais e futuros provedores cloud
│   └── scheduler/                # Integração de agendamento, quando implementada
├── infra/docker/compose.yml     # Bancos para desenvolvimento local
├── tests/fixtures/              # Dados de teste compartilhados e não sensíveis
├── docs/                        # Arquitetura, testes e processo de release
├── .github/workflows/           # CI e releases
├── Cargo.toml                   # Manifesto do Rust Workspace
├── Cargo.lock                   # Versionado para este workspace de aplicações
├── pnpm-workspace.yaml          # Configuração PNPM quando o desktop for criado
├── CHANGELOG.md
└── README.md
```

Regras arquiteturais:

- A CLI e o Tauri são **adaptadores/pontos de entrada**. As regras de negócio ficam nas
  crates Rust compartilhadas, sem duplicação entre as interfaces.
- `core` não deve depender de Tauri, Angular, Clap, Docker ou SDKs específicos de SGBDs.
- Os adaptadores de banco e armazenamento implementam os contratos necessários ao core;
  mantenha as dependências apontando para o domínio. Monte as implementações concretas
  nos limites de cada aplicação.
- O frontend não deve executar comandos de dump nem conter a lógica do motor de backup.
  Ele invoca comandos Tauri específicos e exibe os estados da operação.
- Não adicione API HTTP, serviço agente em segundo plano, fila ou nova crate sem uma
  necessidade real.
- Utilize dependências declaradas no workspace e os lockfiles existentes. Não introduza
  outro gerenciador de pacotes JavaScript.

## 4. Diretrizes para Rust

### Estilo e sistema de tipos

- Siga o Rust idiomático e as Rust API Guidelines; formate com `rustfmt`.
- Use `snake_case` para funções/módulos, `PascalCase` para tipos/traits e
  `SCREAMING_SNAKE_CASE` para constantes.
- Prefira nomes explícitos e significativos, com funções curtas e uma responsabilidade clara.
- Prefira empréstimos (`&T`, `&str`) quando simplificarem o gerenciamento de ownership.
  Evite clones e alocações desnecessários, sem prejudicar a legibilidade.
- Prefira `Option<T>` e `Result<T, E>` a valores sentinela e panics.
- Utilize enums e configurações tipadas para representar capacidades e estados dos SGBDs.
- Mantenha campos privados, exceto quando fizerem parte intencional da API pública.
- **Não escreva comentários no código** (nem `//`, `///` ou `//!`). Nomes, tipos e testes devem
  deixar o comportamento claro. Quando houver algo pertinente a explicar, ou a mudança alterar
  algo grande no sistema (arquitetura, fluxo de backup/restore, formato de dados, segurança),
  registre em `docs/` (por exemplo `docs/arquitetura.md` ou um arquivo novo do tema).
- Evite `unsafe`. Se for inevitável, documente e teste suas invariantes de segurança.

### Erros e observabilidade

- Não use `.unwrap()` em caminhos de execução de produção. Utilize `.expect()` apenas
  para invariantes que razoavelmente não podem falhar, com uma justificativa clara.
- Use `thiserror` para erros tipados de bibliotecas; utilize `anyhow` na composição de erros
  da aplicação se a base do projeto já o adotar.
- Preserve internamente a causa dos erros, mas retorne mensagens sanitizadas para a CLI
  e a interface desktop. Nunca exponha credenciais no contexto de um erro.
- Prefira `tracing` para eventos de diagnóstico e logs estruturados. Mensagens de uso da
  CLI podem ser apresentadas no stdout; diagnósticos devem ir para stderr ou `tracing`.
- Registre IDs de operações, duração, status e metadados seguros. Nunca registre senhas,
  URIs de conexão com segredos, cabeçalhos de autenticação ou conteúdos de backups.

### Assincronismo, I/O e desempenho

- Use Tokio quando operações assíncronas forem necessárias; não introduza async apenas
  para envolver código síncrono simples.
- Não bloqueie o runtime assíncrono com I/O pesado, compressão intensiva em CPU ou
  ferramentas externas síncronas. Use streaming, APIs assíncronas adequadas ou tarefas
  dedicadas para operações bloqueantes.
- Processe backups grandes em streaming, com buffers limitados; não carregue dumps
  completos na memória. Trate cancelamento e saídas parciais explicitamente.
- Utilize controle de fluxo (*backpressure*), concorrência limitada e timeouts quando fizer sentido.
- Faça benchmarks antes de otimizar. Não introduza Rayon, SIMD, alocadores personalizados
  ou cache sem necessidade demonstrada.
- Prefira crates bem mantidas e já utilizadas no projeto. Antes de adicionar dependências,
  avalie manutenção, segurança, licença e complexidade.

## 5. Regras de backup e restauração de bancos

- Cada SGBD possui mecanismos próprios de backup e restauração. Nunca apresente backup
  completo, incremental, diferencial ou restauração seletiva como recursos universais,
  a menos que o adaptador correspondente os implemente e teste.
- Prefira mecanismos oficiais/nativos quando aplicáveis, como `pg_dump` e `pg_restore`
  no PostgreSQL. Não reinvente formatos de dump.
- Verifique a presença, as versões e a compatibilidade das ferramentas nativas antes de começar.
- Execute programas externos com argumentos estruturados do processo, **nunca** interpolando
  entrada não confiável em comandos de shell.
- Não exponha senhas de bancos em argumentos de linha de comando, histórico do shell,
  logs ou arquivos de configuração versionados. Utilize mecanismos seguros de passagem
  de credenciais e remova informações sensíveis das mensagens de erro.
- Antes de operações destrutivas ou custosas, valide conexões, permissões, banco de destino
  selecionado e formato do backup.
- Utilize compressão/criptografia por streaming quando houver suporte e registre os
  metadados necessários à restauração segura e à verificação de integridade.
- Nunca declare um backup bem-sucedido somente porque o processo terminou. Verifique
  código de saída, conclusão da escrita, persistência no armazenamento e metadados registrados.
- Nunca sobrescreva silenciosamente um banco de produção. Prefira restaurar em um destino
  descartável e exija confirmação explícita para restaurações destrutivas.
- Testes de restore devem validar os dados esperados, não somente o código de saída.
- Trate exclusões por retenção, uploads para nuvem e limpeza de arquivos parciais como
  operações sensíveis à segurança e ao risco de perda de dados.

## 6. Desktop Angular + Tauri

**Frontend aprovado:** Angular + TypeScript estrito + Tailwind CSS v4 + Transloco.
React, Ionic e Capacitor não fazem parte do projeto. O desktop está previsto para a
v0.3.0, não para o MVP da CLI PostgreSQL.

### Arquitetura e convenções Angular

- Utilize a versão do Angular instalada e as convenções do Angular CLI. Não presuma
  uma versão nem force upgrades em PRs sem relação com a mudança.
- Utilize **componentes standalone**, rotas por funcionalidade e lazy loading quando
  apropriado. Não introduza NgModules sem necessidade real de compatibilidade.
- Organize o código por funcionalidade e mantenha próximos os arquivos TS, HTML, CSS
  (se necessário) e `*.spec.ts`.
- Utilize `signal()` e `computed()` para estados locais e derivados. Reserve `effect()`
  para efeitos colaterais legítimos, não para copiar estados entre signals.
- Use RxJS para streams assíncronos e integrações baseadas em subscriptions quando útil.
  Gerencie o ciclo de vida com `takeUntilDestroyed()` ou equivalente.
- Prefira serviços tipados para acesso a dados e lógica reutilizável. Componentes cuidam
  de apresentação e interação, não das regras de backup e restauração.
- Mantenha TypeScript e templates em modo estrito. Evite `any` e valide valores `unknown`.
- Siga práticas de acessibilidade: navegação por teclado, labels, avisos de status/erro,
  foco visível e feedback para operações longas.
- Evite imports desnecessários entre funcionalidades. Mova código para `shared/`
  somente quando houver reutilização comprovada; mantenha código específico em sua feature.

### Limites dos diretórios (planejados)

```text
apps/desktop/
├── public/i18n/
│   ├── pt-BR.json
│   └── en-US.json
├── src/
│   ├── app/
│   │   ├── core/
│   │   │   ├── config/
│   │   │   ├── error-handling/
│   │   │   ├── i18n/             # Transloco, serviço de idioma e loader
│   │   │   └── tauri/            # Ponte tipada para comandos Rust
│   │   ├── shared/
│   │   │   ├── ui/
│   │   │   ├── pipes/
│   │   │   └── utils/
│   │   ├── features/
│   │   │   ├── dashboard/
│   │   │   ├── connections/
│   │   │   ├── backups/
│   │   │   ├── restores/
│   │   │   ├── schedules/
│   │   │   ├── history/
│   │   │   └── settings/
│   │   ├── layout/              # Estrutura da aplicação, sidebar e header
│   │   ├── app.config.ts
│   │   └── app.routes.ts
│   └── styles.css               # Import do Tailwind e tokens globais
└── src-tauri/                  # App Rust/Tauri, permissões e comandos
```

- `core/`: configuração global, infraestrutura de tradução, mapeamento de erros e
  ponte Tauri tipada. `shared/`: UI, pipes e funções puras reutilizadas.
  `features/`: páginas, componentes, serviços e modelos de cada funcionalidade.
  `layout/`: estrutura reutilizável do desktop e navegação.
- Não crie um workspace TypeScript separado em `libs/` na raiz até que outro aplicativo
  realmente precise compartilhar pacotes Angular ou TS. O compartilhamento de Rust
  ocorre nas crates da pasta `crates/` da raiz.
- Crie pastas e componentes somente quando houver uso real; evite estruturas vazias e
  classes-base genéricas para necessidades hipotéticas.

### Estilização com Tailwind CSS v4

- Tailwind CSS v4 é a principal solução de estilização. Configure-o pelo Angular CLI
  quando adicionar o desktop, usando `ng add tailwindcss` se houver suporte nas versões
  instaladas, ou a integração documentada com PostCSS.
- Na configuração padrão do Tailwind v4, mantenha `@import "tailwindcss";` em
  `src/styles.css`. Não edite manualmente os arquivos de estilos gerados.
- Defina tokens semânticos de tema em um único lugar e reutilize-os em cores, espaços,
  indicadores de status, foco e tipografia. Evite espalhar cores arbitrárias pela interface.
- **O tema escuro é o padrão**. Ofereça tema claro opcional e persista a preferência caso
  o seletor de tema seja implementado.
- Prefira classes utilitárias do Tailwind. Use CSS de componente para integrações complexas
  ou situações que as utilities não expressem bem. Considere janelas desktop redimensionáveis,
  não somente um tamanho fixo de tela.
- Valide alterações relevantes nos temas escuro e claro e com navegação por teclado.

### Internacionalização (i18n)

- Utilize **Transloco**, com `pt-BR` como idioma padrão e `en-US` como segundo idioma.
  A troca de idioma deve atualizar a interface em execução **sem reiniciar o Tauri**.
- Armazene as traduções em `apps/desktop/public/i18n/` (arquivos estáticos) e a
  configuração, loader e preferência de idioma em `src/app/core/i18n/`.
- Use chaves de tradução estáveis e significativas, como `backup.create` e
  `connection.errors.timeout`. Não escreva mensagens visíveis ao usuário diretamente em
  HTML/TypeScript nem construa chaves arbitrárias a partir de entradas não confiáveis.
- Mantenha as chaves sincronizadas nos dois idiomas, defina explicitamente o fallback e
  teste traduções ausentes. Persista a preferência do usuário em configurações locais
  não sensíveis da aplicação.
- Traduza, conforme necessário, botões, labels, confirmações, indicadores de status,
  validações, mensagens de erro, notificações, datas, números e durações.
- O Rust retorna códigos/identificadores de erro tipados e estáveis; a camada Angular
  traduz as mensagens de apresentação. Mantenha logs técnicos e diagnósticos estáveis,
  sem reutilizar strings traduzidas da interface.
- Adicione ou atualize testes `*.spec.ts` quando modificar seleção de idioma,
  persistência, fallback ou comportamento traduzido.

### Fronteira com o Tauri

- O Angular é uma SPA estática. Não adicione um servidor SSR/Node.js obrigatório
  durante a execução do aplicativo desktop.
- Comandos Tauri devem receber e retornar dados tipados e específicos. Valide as entradas
  novamente em Rust e conceda apenas permissões/capabilities mínimas.
- Não exponha acesso arbitrário ao sistema de arquivos nem execução genérica de comandos à UI.
- Não armazene senhas, tokens de acesso ou conteúdo de backups no armazenamento web
  nem em logs.
- Operações longas não podem bloquear a interface: exponha progresso, conclusão e
  cancelamento por APIs/eventos adequados do gerenciador de jobs em Rust.

## 7. Docker, configuração e segredos

- Utilize Docker Compose em `infra/docker/` para iniciar manualmente os serviços de
  desenvolvimento PostgreSQL, MySQL e MongoDB. SQLite utiliza um arquivo local.
- Não exija servidores de banco instalados diretamente no host para desenvolvimento.
- Use Testcontainers nos testes de integração com bancos isolados. Não reutilize volumes
  ou bancos do Compose de desenvolvimento em testes destrutivos.
- Prefira containers descartáveis e portas atribuídas dinamicamente nos testes.
- Use `.env.example` para documentar valores padrão não sensíveis e placeholders.
  Mantenha `.env`, chaves, backups e dados locais de bancos fora do Git.
- Um arquivo `.env` **não é um cofre de segredos**. Para segredos persistidos no desktop,
  prefira recursos de credenciais do sistema operacional ou outro mecanismo seguro
  configurado explicitamente.
- Credenciais de exemplo devem ficar restritas a ambientes descartáveis de desenvolvimento
  e teste; nunca presuma que são adequadas para produção.
- Nunca exclua silenciosamente volumes Docker, backups de produção ou objetos na nuvem.
  Exija instrução explícita e confirmação segura.

## 8. Testes e verificações de qualidade

Estratégia de testes:

- **Testes unitários Rust:** mantenha módulos pequenos `#[cfg(test)]` próximos do código.
  Teste validações puras, transições de estado, erros, regras de agendamento e lógica
  de nomes de arquivos/metadados sem depender de bancos.
- **Testes de integração Rust:** utilize a pasta `tests/` da crate relevante e
  Testcontainers para comportamento real dos SGBDs, falhas de conexão, backups,
  restaurações e verificações de compatibilidade.
- **Testes E2E da CLI:** execute comandos reais e confira exit codes, stdout, stderr
  e arquivos produzidos. Use bancos descartáveis em cenários destrutivos.
- **Testes unitários/de componentes Angular:** mantenha `*.spec.ts` próximos do código,
  com Angular TestBed e Vitest conforme configurados. Faça mocks das chamadas Tauri e teste
  validação, carregamento, falhas, progresso, estados que dependam do Tailwind (quando
  observáveis) e i18n (`pt-BR`, `en-US`, fallback e idioma persistido).
- **Testes E2E desktop:** adote uma ferramenta compatível com Tauri quando for necessário
  validar fluxos reais entre frontend e Rust. Não apresente testes com mocks como cobertura
  E2E completa.
- Prefira testes de comportamento com o padrão Arrange–Act–Assert. Não faça mock do próprio
  sistema cujo comportamento você pretende validar em um teste de integração.

### Verificação obrigatória após cada implementação

Ao terminar **cada** implementação (issue, correção ou ajuste), antes de mostrar o resultado,
commitar ou abrir PR, rode formatação, lint, build e testes de **todas** as partes existentes
do monorepo, não só das que foram alteradas. Se algum passo falhar, corrija e rode de novo.

Rust, a partir da raiz do repositório:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

Angular, **após a criação da aplicação** em `apps/desktop` (enquanto ela não existir, registre
que a etapa não se aplica):

```bash
pnpm --dir apps/desktop run lint
pnpm --dir apps/desktop run build
pnpm --dir apps/desktop run test:ci
```

São os mesmos scripts usados pela CI; inclua também o script de formatação quando ele existir
no `package.json`.

Scripts de release (quando alterados ou antes de um release):

```bash
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```

Workflows do GitHub Actions, quando alterados: valide com `actionlint`.

No resumo final, liste cada comando executado e o resultado real.

Execute também os scripts configurados para formatação, lint e checagem de tipos quando
existirem. Confirme os nomes reais no `package.json`; não invente comandos.

- Testes que dependem do Docker exigem um daemon disponível. Se estiver ausente,
  execute as verificações independentes e informe quais testes de integração ficaram pendentes.
- Priorize testes de comportamento alterado. Evite testes triviais para cada getter
  ou helper de uma linha sem comportamento observável relevante.
- Nunca edite manualmente os arquivos gerados pelo Angular nem os artefatos de `target/`.

## 9. Documentação, changelog e releases

- Mantenha o README e a documentação afetada atualizados quando houver mudanças de
  comportamento ou de comandos públicos. Documente novas variáveis em `.env.example`.
- Siga Keep a Changelog, com seções como `Added`, `Changed`, `Fixed`, `Removed` e
  `Security` (nomes mantidos para compatibilidade com o padrão do changelog).
- Registre em `Unreleased` apenas mudanças visíveis ao usuário que tenham sido
  **implementadas e verificadas**. Não apresente itens do roadmap como recursos publicados.
- Utilize SemVer e tags Git no formato `vX.Y.Z`, mantendo a política de versão do
  workspace consistente com o workflow de release.
- **Publique releases somente pelo workflow Release do GitHub Actions acionado manualmente
  com `workflow_dispatch` (Run workflow) na `main`**, informando explicitamente a versão.
  Não adicione gatilhos por push de tag nem publicação automática após merge. O workflow
  valida testes, versões e changelog antes de criar a tag, release e arquivos associados.
- Nunca altere versões, crie tags, publique releases ou faça deploy sem solicitação
  explícita ou procedimento de release previamente aprovado.
- Respeite os workflows do GitHub Actions existentes. Não enfraqueça verificações
  para fazer uma alteração não relacionada passar.
- Prefira commits pequenos e descritivos. Não inclua segredos, builds gerados, dumps
  de bancos ou arquivos locais de ambiente.

### Branches, commits e Pull Requests

- **Revisão humana antes de qualquer commit.** O agente não faz `git commit`, `git push` nem abre
  PR por conta própria. Ao terminar, deixe as alterações no working tree, apresente o resumo
  para revisão e só faça commit, push ou PR quando o mantenedor pedir explicitamente, a cada vez.
- **Sempre comece da `main` atualizada.** Nunca crie uma branch a partir de outra branch de
  trabalho, exceto no caso de branch empilhada descrito abaixo:

  ```bash
  git switch main && git pull --ff-only
  git switch -c feat/db-9-pg-dump-runner
  ```

- **Nome da branch**, sempre em inglês e em `kebab-case`:
  `<tipo>/<id-linear>-<descrição-curta>`, por exemplo `feat/db-9-pg-dump-runner` ou
  `fix/db-12-exit-code-on-timeout`. Sem issue no Linear: `<tipo>/<descrição-curta>`, por
  exemplo `docs/branch-and-commit-conventions`.
- **Tipos:** `feat`, `fix`, `docs`, `test`, `chore`, `ci`, `refactor`, `perf`.
- **Uma branch e um PR por issue.** Não misture issues no mesmo PR.
- **Branch empilhada:** só quando a issue depende de outra ainda não mergeada. Crie a branch a
  partir da branch da dependência, abra o PR contra a `main` (a CI só roda em PRs para a `main`)
  e avise no PR qual deve ser mergeado primeiro.
- **Mensagens de commit** em inglês, no padrão [Conventional Commits](https://www.conventionalcommits.org/):
  `<tipo>(<escopo opcional>): <resumo no imperativo>`, com no máximo ~72 caracteres no resumo,
  por exemplo `feat(cli): add backup command` ou `fix(ci): check for the angular app inside a step`.
  Use o corpo da mensagem para explicar o porquê da mudança.
- **Título do PR** em inglês, no mesmo formato, com o ID da issue no final:
  `feat(cli): add backup command (DB-10)`. O corpo segue `.github/pull_request_template.md` e
  inclui `Closes DB-N` para vincular e fechar a issue do Linear no merge.
- **Merge** com Squash & Merge depois da CI verde. Depois de um merge, a próxima branch volta
  a partir da `main` atualizada.
- **Linear após o merge.** Ao confirmar que o PR foi mergeado (por exemplo, com `gh pr view <N>`
  ou o merge commit na `main`), atualize o card no Linear: mova para **Done**, anexe o link do PR
  se ainda não estiver lá e comente um resumo curto do que foi entregue e dos riscos restantes.
  Não dependa do `Closes DB-N` para fechar o card, porque a integração nem sempre o move. Nunca
  marque como Done um card cujo PR ainda não foi mergeado.

## 10. Antes de concluir qualquer tarefa

1. Confirme que a alteração atende diretamente à solicitação.
2. Verifique se não existem mudanças em arquivos não relacionados nem código de debug restante.
3. Revise entradas/saídas sensíveis e caminhos destrutivos de backup/restauração.
4. Execute a verificação obrigatória da seção 8 (formatação, lint, build e testes de Rust e Angular).
5. Atualize os testes e a documentação específicos quando necessário.
6. Avalie se o `CHANGELOG.md` precisa receber uma entrada em `Unreleased`.
7. Resuma os arquivos alterados, os resultados observados, os comandos realmente executados,
   as limitações e os riscos restantes.
8. Pare para revisão: não faça commit, push nem PR sem pedido explícito do mantenedor.

## 11. Referências adicionais

- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- [Cargo Workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)
- [Clippy](https://doc.rust-lang.org/clippy/)
- [Angular Testing](https://angular.dev/guide/testing)
- [Angular Style Guide](https://angular.dev/style-guide)
- [Angular Tailwind Integration](https://angular.dev/guide/tailwind)
- [Transloco](https://jsverse.github.io/transloco/)
- [Tauri Frontend Configuration](https://tauri.app/start/frontend/)
- [Testcontainers for Rust](https://docs.rs/testcontainers/)
- [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
- [Semantic Versioning](https://semver.org/)
