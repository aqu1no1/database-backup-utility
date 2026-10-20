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
