# Configuração local

1. Copie `.env.example` para `.env` e altere as senhas **somente locais**.
2. Inicie o banco de desenvolvimento:

```bash
docker compose --env-file .env -f infra/docker/compose.yml up -d postgres
```

3. Para iniciar os demais bancos em desenvolvimento, se necessário:

```bash
docker compose --env-file .env -f infra/docker/compose.yml \
  --profile mysql --profile mongodb up -d
```

## Variáveis

| Variável | Finalidade | Exemplo não secreto |
| --- | --- | --- |
| POSTGRES_USER | Usuário de desenvolvimento | postgres |
| POSTGRES_PASSWORD | Senha exclusivamente local | Trocar o valor do exemplo |
| POSTGRES_DB | Banco de desenvolvimento | my_database |
| MYSQL_ROOT_PASSWORD | Senha root MySQL dev | Trocar o valor do exemplo |
| MYSQL_DATABASE | Banco MySQL dev | my_database |
| MYSQL_USER | Usuário MySQL dev | backup_dev |
| MYSQL_PASSWORD | Senha MySQL dev | Trocar o valor do exemplo |
| MONGO_USER | Usuário MongoDB dev | admin |
| MONGO_PASSWORD | Senha MongoDB dev | Trocar o valor do exemplo |

- Todos os serviços publicados na máquina usam `127.0.0.1`, não são expostos na rede pública.
- `.env` está no `.gitignore`; não contém dados reais de produção.
- Testcontainers usa credenciais efêmeras de teste e não lê esse `.env`.
- Em produção, segredos serão tratados por provedores apropriados a CLI interativa,
  agendamento sem sessão gráfica e Tauri; `.env` não equivale a um cofre.

## Variáveis da CLI

| Variável | Finalidade |
| --- | --- |
| `DBU_PG_PASSWORD` | Senha usada pelo `test-connection` em scripts e agendamentos. Sem ela, a CLI pede a senha sem eco no terminal. |
| `DBU_DATA_DIR` | Diretório do histórico SQLite, no lugar do padrão do sistema. Útil para testes e uso portátil. |

- A CLI não lê o `.env`; exporte as variáveis no shell ou no agendador.
- A senha nunca é aceita como argumento, porque argumentos ficam visíveis em `ps` e no histórico do shell.
- Variáveis de ambiente ficam visíveis para o próprio usuário e o root (`/proc/<pid>/environ`).
  Um provedor via keyring do sistema está previsto para uma issue futura.

## TLS no teste de conexão

| `--tls` | `localhost` / `127.0.0.1` / `::1` | Outros hosts |
| --- | --- | --- |
| `auto` (padrão) | Sem TLS | TLS com certificado validado |
| `disable` | Sem TLS | Recusado antes de conectar |
| `verify-full` | TLS com certificado validado | TLS com certificado validado |

A validação usa os certificados raiz do sistema operacional.

## Histórico local (SQLite)

O arquivo `history.sqlite3` fica no diretório de dados do usuário:

| Sistema | Caminho padrão |
| --- | --- |
| Linux | `~/.local/share/database-backup-utility/` |
| macOS | `~/Library/Application Support/dev.aqu1no1.database-backup-utility/` |
| Windows | `%APPDATA%\aqu1no1\database-backup-utility\data\` |

As migrations rodam ao abrir o histórico e só aplicam o que falta (versão em `PRAGMA user_version`).
O schema está em `docs/diagramas/modelo-de-dados.md`.

## Ferramentas `pg_dump` / `pg_restore`

O backup usa as ferramentas oficiais do PostgreSQL, executadas de dois jeitos:

| Runner | Quando usar | Como funciona |
| --- | --- | --- |
| `native` | Cliente PostgreSQL instalado na máquina | Usa `pg_dump`/`pg_restore` do `PATH`. A major do cliente precisa ser maior ou igual à do servidor. |
| `docker` | Sem cliente instalado, ou cliente antigo | Roda `postgres:<major do servidor>` com `docker run --rm`; a versão sempre bate com a do servidor. |
| `auto` (padrão) | — | `native` se existir e for compatível; senão `docker`. |

- **Senha:** gravada num `.pgpass` temporário com permissão `0600`, apontado por `PGPASSFILE` e
  apagado no fim, inclusive em falha ou cancelamento. No modo Docker o arquivo é montado só para
  leitura. A senha nunca aparece nos argumentos do processo nem nas mensagens de erro.
- **TLS:** segue a mesma regra do `test-connection` (`PGSSLMODE=disable` em `localhost`,
  `verify-full` nos demais). No Docker, os certificados do sistema são montados no container.
- **Rede no Docker:** para `localhost` o container usa `--network host`, porque o Compose publica
  o PostgreSQL só em `127.0.0.1`. No macOS e no Windows isso exige o host networking do
  Docker Desktop habilitado. Para hosts remotos é usada a rede padrão.
- **Cancelamento:** o container recebe um nome `dbu-pg-<id>` e é removido com
  `docker rm --force` ao cancelar.

## Pasta dos backups

- Padrão: `backups/` dentro da pasta de dados (ao lado de `history.sqlite3`); `--output-dir`
  escolhe outra. Se `DBU_DATA_DIR` estiver definido, o padrão vira `$DBU_DATA_DIR/backups`.
- Quando a CLI cria a pasta, ela fica com permissão `0700`; dumps e metadados ficam com `0600`.
- Um backup nunca sobrescreve um arquivo existente.
