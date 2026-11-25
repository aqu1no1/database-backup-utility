# Desktop (Angular)

O app desktop fica em `apps/desktop`: Angular 22 standalone, TypeScript estrito, sem SSR,
gerenciado por PNPM no workspace da raiz (`pnpm-workspace.yaml` e `pnpm-lock.yaml`). Já existem o
design system (Tailwind CSS v4, tema escuro e claro, componentes compartilhados) e o shell com a
sidebar (DB-26), o i18n com Transloco (DB-25), o início do contrato com o backend, simulado (DB-52),
e as telas Configurações, Painel, Histórico e Agendamentos. O Tauri e as demais telas entram nos
próximos cards do milestone v0.4.0 do épico DB-16 (DB-45, DB-24 e DB-48 a DB-50).

O design de referência fica no repositório de designs, em `database-backup-utility/` (telas em
`design-source/`, tokens em `angular/`).

## Estrutura

```text
apps/desktop/
├── public/                     # Arquivos estáticos copiados para o build
├── src/
│   ├── app/
│   │   ├── core/theme/         # ThemeService: tema escuro (padrão) ou claro, salvo em dbu-theme
│   │   ├── layout/
│   │   │   ├── shell/          # Sidebar + <router-outlet>
│   │   │   └── sidebar/        # Marca, navegação principal e Configurações
│   │   ├── features/
│   │   │   └── dashboard/      # Painel, carregado sob demanda
│   │   ├── shared/
│   │   │   ├── ui/             # Componentes do design system
│   │   │   ├── pipes/          # fileSize, duration, dateTime, shortKey
│   │   │   └── utils/          # Funções puras: formatação e validação de chave age
│   │   ├── app.config.ts       # Providers globais (router, erros, tema)
│   │   ├── app.routes.ts       # Rotas de topo; cada feature com loadComponent/loadChildren
│   │   └── app.ts              # Componente raiz, só renderiza o shell
│   ├── index.html
│   ├── main.ts
│   └── styles.css              # Tailwind, tokens de cor dos dois temas, fontes e utilities
├── .postcssrc.json             # Plugin do Tailwind CSS v4 para o build do Angular
├── angular.json
├── eslint.config.js
└── package.json
```

| Diretório | Responsabilidade | Situação |
| --- | --- | --- |
| `src/app/layout/` | Estrutura do app: shell e sidebar | Criado (`shell`, `sidebar`) |
| `src/app/features/<nome>/` | Páginas, componentes, serviços e modelos de uma funcionalidade | Criado (`dashboard`) |
| `src/app/core/` | Tema, i18n, configuração global, mapeamento de erros e bridge Tauri | Criado (`theme`); i18n no DB-25, backend no DB-52 |
| `src/app/shared/` | UI, pipes e funções puras usadas por mais de uma feature | Criado (`ui`, `pipes`, `utils`) |
| `public/i18n/` | Traduções `pt-BR.json` e `en-US.json` | Criar com o Transloco (DB-25) |
| `src-tauri/` | App Rust/Tauri, capabilities e comandos | Criado (DB-45), ainda sem comandos |

Regras:

- Pastas só nascem com o primeiro arquivo que as usa; não há diretórios vazios reservados.
- Cada feature é carregada sob demanda a partir de `app.routes.ts`. Uma feature não importa outra;
  o que for usado por duas ou mais vai para `shared/`.
- Os `*.spec.ts` ficam ao lado do arquivo testado.
- O seletor dos componentes usa o prefixo `app-` (verificado pelo ESLint).
- O frontend não contém regra de backup ou restore; ele chama comandos Tauri e mostra o estado.

## Estilo e tema

- **Tailwind CSS v4** pelo PostCSS (`.postcssrc.json`). A paleta padrão do Tailwind é removida
  (`--color-*: initial`): só existem os tokens do design, como `bg-surface`, `text-fg-muted`,
  `border-border` e `bg-danger-bg`.
- **Tema:** escuro é o padrão. O claro é ligado por `data-theme="light"` no `<html>`, que redefine
  as mesmas variáveis. O `ThemeService` aplica o tema na inicialização e guarda a escolha em
  `localStorage` (`dbu-theme`), que é uma preferência não sensível. Não há opção "seguir o sistema".
- **Fontes:** IBM Plex Sans (texto) e JetBrains Mono (dados, IDs, caminhos) vêm dos pacotes
  `@fontsource`, só com os subconjuntos latin e latin-ext. Elas entram no build como arquivos
  locais, sem Google Fonts, porque o app precisa funcionar offline e o CSP do Tauri bloqueia origem
  externa.
- **Utilities do projeto** (em `styles.css`): `overline` (sobretítulo em mono), `card` (superfície
  com borda), `card-inset` (fundo do canvas), `control` (campo de 44 px) e a classe `data-table`
  para tabelas.
- **Movimento:** a barra de progresso indeterminada para de animar com
  `prefers-reduced-motion: reduce`.

## Componentes (`shared/ui`)

Nenhum componente tem texto fixo: rótulos e mensagens chegam por input ou por conteúdo projetado,
para o i18n (DB-25) entregar as traduções. Estados desabilitados sempre mostram o motivo.

| Componente | Uso |
| --- | --- |
| `button[appButton]`, `a[appButton]` | Variantes `primary`, `secondary`, `destructive` e `text`; tamanho `sm` para tabelas |
| `app-status-badge` | Status `success`, `running`, `warning`, `danger` e `planned`; o símbolo é decorativo |
| `app-tag` | Etiqueta com contorno: criptografado, comprimido, SGBD |
| `app-stepper` | Etapas numeradas; a atual com `aria-current="step"` |
| `app-field` + `appControl` | Rótulo, campo, ajuda e erro, ligados por `for`, `aria-describedby` e `aria-invalid` |
| `app-check` | Checkbox com rótulo e motivo quando desabilitado |
| `app-option-card` | Card de opção com rádio: selecionado, normal ou desabilitado com motivo |
| `app-segmented` | Escolha entre poucas opções (`aria-pressed`), como barra ou como filtros em pílula, com a dica da opção escolhida |
| `app-file-picker` | Caminho escolhido e botão; só emite `pick`, quem usa abre o diálogo |
| `app-public-key-list` | Lista editável de chaves públicas age, com validação de formato e duplicata |
| `app-metric-card`, `app-info-card`, `app-note` | Cards de métrica, de informação e nota tracejada |
| `app-alert` | Erro (`role="alert"`, com o `code` estável) ou atenção |
| `app-progress` | Progresso determinado ou indeterminado (texto da etapa e dos bytes gravados) |
| `app-dialog` | Modal sobre o `<dialog>` nativo (foco preso, Esc fecha) |
| `app-page-header` | Sobretítulo, `h1` e ações da página |

Os pipes `fileSize`, `duration` e `dateTime` formatam pelo idioma do `LanguageService`. `shortKey` encurta chaves públicas (12 primeiros e 6 últimos caracteres).

## Tauri

`src-tauri/` (Tauri 2, crate `database-backup-desktop`) abre o build do Angular numa janela. Ainda não
há comandos Rust: o app usa o backend simulado até o DB-24, e nenhum instalador é gerado.

- **Capabilities** (`capabilities/main.json`): só `dialog:allow-open` e `dialog:allow-save` na janela
  `main`. Sem `core:default`, shell ou filesystem. O `tauri-plugin-dialog` depende do
  `tauri-plugin-fs`, que não é registrado nem tem permissão. A cópia da chave pública (K1) usará
  `navigator.clipboard`, sem plugin.
- **CSP:** nenhuma origem externa, script só do app. `style-src` aceita `'unsafe-inline'` porque o
  Angular insere os estilos dos componentes em execução; `dangerousDisableAssetCspModification` em
  `style-src` impede o Tauri de acrescentar um nonce, que anularia essa permissão. O `inlineCritical`
  do Angular fica desligado porque depende de um handler inline, que a CSP bloqueia. A `devCsp`, usada
  só no `tauri dev`, libera também o script inline e o WebSocket do `ng serve`.
- **Ícones:** gerados de `icons/icon.svg` do repositório de designs com `pnpm tauri icon`.
- **Linux:** compilar exige `libwebkit2gtk-4.1-dev` e `libgtk-3-dev`. Por isso os comandos Cargo do
  workspace usam `--exclude database-backup-desktop`, e a CI compila o app no job `Tauri / Build & Smoke`.
  Esse job confirma que o processo continua aberto por 20 s; não confere o conteúdo da tela.

## Comandos

A partir da raiz do repositório:

```bash
pnpm install --frozen-lockfile
pnpm --dir apps/desktop run start         # servidor de desenvolvimento em http://localhost:4200
pnpm --dir apps/desktop tauri dev            # app Tauri com o servidor de desenvolvimento
pnpm --dir apps/desktop run lint          # ESLint (angular-eslint), inclusive acessibilidade dos templates
pnpm --dir apps/desktop run test:ci       # Vitest + TestBed, uma execução
pnpm --dir apps/desktop run build         # build de produção em apps/desktop/dist
pnpm --dir apps/desktop run format:check  # Prettier
```

A CI (`Angular / Lint, Test & Build`) roda lint, testes e build com PNPM 10 e Node 24. Use PNPM 10
localmente (`npx pnpm@10`) para o lockfile continuar no formato que a CI lê. Se o `node_modules`
tiver sido instalado por outra versão do PNPM, rode `npx pnpm@10 install --frozen-lockfile` antes.
Os scripts de build de `esbuild`, `lmdb`, `msgpackr-extract` e `@parcel/watcher` ficam desligados
(`ignoredBuiltDependencies`); o build usa os binários pré-compilados de cada plataforma.
