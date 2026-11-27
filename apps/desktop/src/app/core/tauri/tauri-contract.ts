import {
  AppError,
  Connection,
  DATABASE_ENGINES,
  DatabaseEngine,
  ERROR_CODES,
  ErrorCode,
  HistoryItem,
  OperationKind,
  OperationStatus,
  Settings,
  ToolRunner,
} from '../backend/backend.models';

type Fields = Readonly<Record<string, unknown>>;

const KINDS: readonly OperationKind[] = ['backup', 'restore'];
const STATUSES: readonly OperationStatus[] = [
  'running',
  'verified',
  'completed',
  'failed',
  'cancelled',
];
const RUNNERS: readonly ToolRunner[] = ['native', 'docker'];
const SERVER_ENGINES = DATABASE_ENGINES.filter(
  (engine): engine is Exclude<DatabaseEngine, 'sqlite'> => engine !== 'sqlite',
);

function malformed(): AppError {
  return new AppError('internal');
}

function record(value: unknown): Fields {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw malformed();
  }
  return value as Fields;
}

function list(value: unknown): readonly unknown[] {
  if (!Array.isArray(value)) {
    throw malformed();
  }
  return value;
}

function text(fields: Fields, key: string): string {
  const value = fields[key];
  if (typeof value !== 'string') {
    throw malformed();
  }
  return value;
}

function oneOf<T extends string>(fields: Fields, key: string, allowed: readonly T[]): T {
  const value = fields[key];
  if (typeof value !== 'string' || !(allowed as readonly string[]).includes(value)) {
    throw malformed();
  }
  return value as T;
}

function port(fields: Fields): number {
  const value = fields['port'];
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 1 || value > 65535) {
    throw malformed();
  }
  return value;
}

export function parseHistory(value: unknown): readonly HistoryItem[] {
  return list(value).map((entry) => {
    const fields = record(entry);
    return {
      id: text(fields, 'id'),
      kind: oneOf(fields, 'kind', KINDS),
      engine: oneOf(fields, 'engine', DATABASE_ENGINES),
      source: text(fields, 'source'),
      destination: text(fields, 'destination'),
      startedAt: text(fields, 'startedAt'),
      status: oneOf(fields, 'status', STATUSES),
      errorCode: fields['errorCode'] === null ? null : oneOf(fields, 'errorCode', ERROR_CODES),
    };
  });
}

export function parseConnections(value: unknown): readonly Connection[] {
  return list(value).map((entry) => {
    const fields = record(entry);
    return {
      name: text(fields, 'name'),
      engine: oneOf(fields, 'engine', SERVER_ENGINES),
      host: text(fields, 'host'),
      port: port(fields),
      database: text(fields, 'database'),
      user: text(fields, 'user'),
    };
  });
}

export function parseSettings(value: unknown): Settings {
  const fields = record(value);
  return {
    runner: oneOf(fields, 'runner', RUNNERS),
    backupDirectory: text(fields, 'backupDirectory'),
  };
}

function textParams(value: unknown): Readonly<Record<string, string>> {
  if (typeof value !== 'object' || value === null) {
    return {};
  }
  return Object.fromEntries(
    Object.entries(value).filter(
      (entry): entry is [string, string] => typeof entry[1] === 'string',
    ),
  );
}

export function toAppError(error: unknown): AppError {
  if (error instanceof AppError) {
    return error;
  }
  if (typeof error === 'object' && error !== null) {
    const { code, params } = error as Fields;
    if (typeof code === 'string' && (ERROR_CODES as readonly string[]).includes(code)) {
      return new AppError(code as ErrorCode, textParams(params));
    }
  }
  return new AppError('internal');
}
