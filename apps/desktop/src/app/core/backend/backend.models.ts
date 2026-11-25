export const DATABASE_ENGINES = ['postgresql', 'mysql', 'mariadb', 'mongodb', 'sqlite'] as const;
export type DatabaseEngine = (typeof DATABASE_ENGINES)[number];

export const ENGINE_LABELS: Record<DatabaseEngine, string> = {
  postgresql: 'PostgreSQL',
  mysql: 'MySQL',
  mariadb: 'MariaDB',
  mongodb: 'MongoDB',
  sqlite: 'SQLite',
};

export const ERROR_CODES = [
  'auth_failed',
  'database_not_found',
  'unreachable',
  'tls_failed',
  'tool_unavailable',
  'engine_mismatch',
  'backup_failed',
  'cancelled',
  'backup_missing',
  'backup_corrupted',
  'checksum_missing',
  'backup_incompatible',
  'age_key_mismatch',
  'restore_failed',
] as const;
export type ErrorCode = (typeof ERROR_CODES)[number];

export class AppError extends Error {
  constructor(
    readonly code: ErrorCode,
    readonly params: Readonly<Record<string, string>> = {},
  ) {
    super(code);
    this.name = 'AppError';
  }
}

export interface ServerConnection {
  readonly name: string;
  readonly engine: Exclude<DatabaseEngine, 'sqlite'>;
  readonly host: string;
  readonly port: number;
  readonly database: string;
  readonly user: string;
}

export interface SqliteConnection {
  readonly name: string;
  readonly engine: 'sqlite';
  readonly path: string;
}

export type Connection = ServerConnection | SqliteConnection;

export type OperationKind = 'backup' | 'restore';
export type OperationStatus = 'running' | 'verified' | 'completed' | 'failed';

export interface HistoryItem {
  readonly id: string;
  readonly kind: OperationKind;
  readonly engine: DatabaseEngine;
  readonly source: string;
  readonly destination: string;
  readonly startedAt: string;
  readonly status: OperationStatus;
  readonly errorCode: ErrorCode | null;
}

export type ToolRunner = 'native' | 'docker';

export interface Settings {
  readonly runner: ToolRunner;
  readonly backupDirectory: string;
}
