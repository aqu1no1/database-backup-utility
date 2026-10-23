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
| `DBU_PG_PASSWORD` | Senha usada por `connection test`, `backup run` e `restore run` em scripts. |
| `DBU_PG_PASSWORD_FILE` | Caminho de um arquivo com a senha (equivale a `--password-file`). Recomendado para agendadores. |
| `DBU_MYSQL_PASSWORD` | O mesmo que `DBU_PG_PASSWORD`, para `--engine mysql`. |
| `DBU_MYSQL_PASSWORD_FILE` | O mesmo que `DBU_PG_PASSWORD_FILE`, para `--engine mysql`. |
| `DBU_DATA_DIR` | Diretório do histórico SQLite, no lugar do padrão do sistema. Útil para testes e uso portátil. |
| `RUST_LOG` | Filtro dos logs no stderr (ex.: `info`, `database_backup=debug`) quando `--log-level` não é informado. Padrão: `warn`. |

- A CLI não lê o `.env`; exporte as variáveis no shell ou no agendador.

## Credenciais

A senha do PostgreSQL vem de **uma** destas fontes (no MySQL, as variáveis são `DBU_MYSQL_PASSWORD_FILE`
e `DBU_MYSQL_PASSWORD`; as `DBU_PG_*` são ignoradas com `--engine mysql` e vice-versa):

| Fonte | Quando usar |
| --- | --- |
| `--password-file <ARQUIVO>` | Agendadores e scripts. |
| `DBU_PG_PASSWORD_FILE` | O mesmo que a flag, via ambiente (ex.: unit do systemd). |
| `DBU_PG_PASSWORD` | Scripts curtos. Fica visível em `/proc/<pid>/environ` para o próprio usuário e o root. |
| Prompt sem eco | Uso interativo, quando nenhuma das anteriores está definida e o stdin é um terminal. |

- **Mais de uma fonte definida é erro** (exit 2), e a mensagem lista quais foram encontradas. Isso
  evita usar sem querer a senha de outro ambiente que ficou exportada no shell.
- Sem nenhuma fonte e sem terminal (cron, systemd, CI), a CLI sai com 2 e explica as opções.
- A senha **nunca** é aceita como argumento: argumentos ficam visíveis em `ps` e no histórico do shell.
- O histórico SQLite guarda só de onde a senha veio (`connections.secret_ref`: `prompt`,
  `env:DBU_PG_PASSWORD` ou `file:<caminho>`), nunca o valor. Mensagens de erro citam o caminho do
  arquivo, nunca o conteúdo.

Regras do arquivo de senha (`crates/core/src/secret.rs`):

- arquivo regular, sem link simbólico;
- no Linux/macOS, nenhuma permissão para grupo ou outros (`600` ou `400`). No Windows essa checagem
  não existe; proteja o arquivo pelas ACLs;
- até 4 KiB, em UTF-8 e não vazio;
- só a quebra de linha final (`\n` ou `\r\n`) é removida; espaços fazem parte da senha.

```bash
install -m 600 /dev/null ~/.config/dbu/pg.pass
printf '%s\n' 'a-senha' > ~/.config/dbu/pg.pass
DBU_PG_PASSWORD_FILE=~/.config/dbu/pg.pass database-backup backup run --database app --user backup
```

No MySQL, a senha lida por essas fontes vai para o `mysqldump`/`mysql` num arquivo de opções
temporário (`[client]`, `0600`, apagado ao fim do processo) passado com `--defaults-extra-file`.
`MYSQL_PWD` é removida do ambiente do processo filho. No modo Docker, o arquivo é montado só leitura.

### Keyring do sistema (futuro)

O contrato `SecretProvider` (core) permite adicionar um adaptador para o cofre do sistema
operacional (Secret Service no Linux, Keychain no macOS, Credential Manager no Windows) sem mudar os
comandos. Não está no MVP por causa do uso sem terminal: o Secret Service exige uma sessão D-Bus com o
chaveiro desbloqueado, o que cron e serviços do systemd normalmente não têm. Para agendamento, o
arquivo de senha com permissão `600` continua sendo o caminho suportado.

### Por que não a crate `secrecy`

O tipo `Password` do core já esconde o valor no `Debug` (`Password(***)`) e zera os bytes no `Drop`.
A crate `secrecy` traria o mesmo resultado com uma dependência a mais. Reavaliar se surgirem outros
tipos de segredo, como tokens de nuvem.

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
- **TLS:** segue a mesma regra do `connection test` (`PGSSLMODE=disable` em `localhost`,
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

## Restore e permissões

- O usuário do restore precisa de `CREATEDB` (o restore sempre cria um banco, novo ou temporário).
- Para substituir um banco existente, o usuário precisa poder renomeá-lo e encerrar as conexões
  dele (ser dono do banco ou superusuário).
- O banco anterior fica como `<destino>__previous_<AAAAMMDDTHHMMSSZ>`. Depois de conferir o
  restore, apague-o manualmente:

  ```sql
  DROP DATABASE "my_database__previous_20261020T190509Z";
  ```

- O restore usa `--no-owner --no-privileges`: os objetos ficam com o usuário do restore como dono
  e as permissões (GRANTs) do banco de origem não são reaplicadas.
