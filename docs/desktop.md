# Desktop (Angular)

O app desktop fica em `apps/desktop`: Angular 22 standalone, TypeScript estrito, sem SSR,
gerenciado por PNPM no workspace da raiz (`pnpm-workspace.yaml` e `pnpm-lock.yaml`). Por enquanto
existe só o esqueleto Angular. Tauri, Tailwind CSS v4 e Transloco entram nos próximos cards do
milestone v0.3.0 do épico DB-16 (DB-24, DB-25, DB-26).

## Estrutura

```text
apps/desktop/
├── public/                     # Arquivos estáticos copiados para o build
├── src/
│   ├── app/
│   │   ├── layout/shell/       # Shell: header, navegação e <router-outlet>
│   │   ├── features/
│   │   │   └── dashboard/      # Página inicial, carregada sob demanda
│   │   ├── app.config.ts       # Providers globais (router, erros)
│   │   ├── app.routes.ts       # Rotas de topo; cada feature com loadComponent/loadChildren
│   │   └── app.ts              # Componente raiz, só renderiza o shell
│   ├── index.html
│   ├── main.ts
│   └── styles.css
├── angular.json
├── eslint.config.js
└── package.json
```

| Diretório | Responsabilidade | Situação |
| --- | --- | --- |
| `src/app/layout/` | Estrutura do app: shell, sidebar e header | Criado (`shell`) |
| `src/app/features/<nome>/` | Páginas, componentes, serviços e modelos de uma funcionalidade | Criado (`dashboard`) |
| `src/app/core/` | i18n, configuração global, mapeamento de erros e bridge Tauri | Criar com o primeiro uso (DB-24, DB-25) |
| `src/app/shared/` | UI, pipes e funções puras usadas por mais de uma feature | Criar quando houver reúso real |
| `public/i18n/` | Traduções `pt-BR.json` e `en-US.json` | Criar com o Transloco (DB-25) |
| `src-tauri/` | App Rust/Tauri, capabilities e comandos | Criar com o Tauri |

Regras:

- Pastas só nascem com o primeiro arquivo que as usa; não há diretórios vazios reservados.
- Cada feature é carregada sob demanda a partir de `app.routes.ts`. Uma feature não importa outra;
  o que for usado por duas ou mais vai para `shared/`.
- Os `*.spec.ts` ficam ao lado do arquivo testado.
- O seletor dos componentes usa o prefixo `app-` (verificado pelo ESLint).
- O frontend não contém regra de backup ou restore; ele chama comandos Tauri e mostra o estado.

## Comandos

A partir da raiz do repositório:

```bash
pnpm install --frozen-lockfile
pnpm --dir apps/desktop run start         # servidor de desenvolvimento em http://localhost:4200
pnpm --dir apps/desktop run lint          # ESLint (angular-eslint), inclusive acessibilidade dos templates
pnpm --dir apps/desktop run test:ci       # Vitest + TestBed, uma execução
pnpm --dir apps/desktop run build         # build de produção em apps/desktop/dist
pnpm --dir apps/desktop run format:check  # Prettier
```

A CI (`Angular / Lint, Test & Build`) roda lint, testes e build com PNPM 10 e Node 24. Use PNPM 10
localmente (`npx pnpm@10`) para o lockfile continuar no formato que a CI lê. Os scripts de build de
`esbuild`, `lmdb`, `msgpackr-extract` e `@parcel/watcher` ficam desligados
(`ignoredBuiltDependencies`); o build usa os binários pré-compilados de cada plataforma.
