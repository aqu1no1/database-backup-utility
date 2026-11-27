import { firstValueFrom } from 'rxjs';
import { AppError, ErrorCode } from './backend.models';
import { MockBackend } from './mock-backend';

describe('MockBackend', () => {
  let backend: MockBackend;

  beforeEach(() => {
    backend = new MockBackend();
    backend.latencyMs = 0;
  });

  it('lists the connections of the design, one per engine', async () => {
    const connections = await firstValueFrom(backend.listConnections());

    expect(connections.map(({ engine }) => engine)).toEqual([
      'postgresql',
      'mysql',
      'mariadb',
      'mongodb',
      'sqlite',
    ]);
  });

  it('keeps the saved settings', async () => {
    await firstValueFrom(backend.saveSettings({ runner: 'docker', backupDirectory: '/tmp' }));

    expect(await firstValueFrom(backend.getSettings())).toEqual({
      runner: 'docker',
      backupDirectory: '/tmp',
    });
  });

  it.each<ErrorCode>([
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
    'invalid_input',
    'confirmation_required',
    'internal',
  ])('fails with %s when asked to and recovers afterwards', async (code) => {
    backend.failWith('listHistory', code);

    await expect(firstValueFrom(backend.listHistory())).rejects.toEqual(new AppError(code));

    backend.recover('listHistory');
    expect((await firstValueFrom(backend.listHistory())).length).toBeGreaterThan(0);
  });
});
