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
