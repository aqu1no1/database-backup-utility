# Modelo do histórico interno (proposta)

> **Ainda não implementado**. A issue DB-8 definirá o esquema real e as migrações SQLite.

```mermaid
erDiagram
    CONNECTIONS ||--o{ BACKUP_JOBS : possui
    BACKUP_JOBS ||--o{ BACKUP_EXECUTIONS : executa
    BACKUP_EXECUTIONS ||--o{ BACKUP_ARTIFACTS : gera
    CONNECTIONS {
        string id PK
        string display_name
        string engine
        string settings_json
    }
    BACKUP_JOBS {
        string id PK
        string connection_id FK
        string job_name
    }
    BACKUP_EXECUTIONS {
        string id PK
        string job_id FK
        string status
        datetime started_at
        datetime finished_at
    }
    BACKUP_ARTIFACTS {
        string id PK
        string execution_id FK
        string location
        string format
        string checksum
    }
```

- Credenciais de bancos **não** serão armazenadas em texto puro no histórico.
- Schedules e delivery logs serão adicionados somente quando usados por funcionalidades reais.
