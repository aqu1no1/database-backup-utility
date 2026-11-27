import { TestBed } from '@angular/core/testing';
import { firstValueFrom } from 'rxjs';
import { AppError, ERROR_CODES } from '../backend/backend.models';
import connections from './contract/connections.json';
import errorCodes from './contract/error-codes.json';
import history from './contract/history.json';
import settings from './contract/settings.json';
import { TAURI_INVOKE, TauriBackend, TauriInvoke } from './tauri-backend';

interface Call {
  readonly command: string;
  readonly args?: Record<string, unknown>;
}

describe('TauriBackend', () => {
  let calls: Call[];

  function backend(respond: (command: string) => Promise<unknown>): TauriBackend {
    calls = [];
    const invoke: TauriInvoke = (command, args) => {
      calls.push({ command, args });
      return respond(command);
    };
    TestBed.configureTestingModule({
      providers: [TauriBackend, { provide: TAURI_INVOKE, useValue: invoke }],
    });
    return TestBed.inject(TauriBackend);
  }

  it('accepts exactly what the Rust commands return', async () => {
    const responses: Record<string, unknown> = {
      history_list: history,
      connections_list: connections,
      settings_get: settings,
      settings_set: settings,
    };
    const tauri = backend(async (command) => responses[command]);

    expect(await firstValueFrom(tauri.listHistory())).toEqual(history);
    expect(await firstValueFrom(tauri.listConnections())).toEqual(connections);
    expect(await firstValueFrom(tauri.getSettings())).toEqual(settings);
    expect(
      await firstValueFrom(
        tauri.saveSettings({ runner: 'docker', backupDirectory: '/srv/backups' }),
      ),
    ).toEqual(settings);
    expect(calls.map(({ command }) => command)).toEqual([
      'history_list',
      'connections_list',
      'settings_get',
      'settings_set',
    ]);
    expect(calls.at(-1)?.args).toEqual({
      settings: { runner: 'docker', backupDirectory: '/srv/backups' },
    });
  });

  it('knows every error code the Rust side can send', () => {
    expect([...ERROR_CODES]).toEqual(errorCodes);
  });

  it('turns a command error into an AppError with its params', async () => {
    const tauri = backend(() =>
      Promise.reject({ code: 'invalid_input', params: { field: 'backupDirectory' } }),
    );

    await expect(firstValueFrom(tauri.getSettings())).rejects.toEqual(
      new AppError('invalid_input', { field: 'backupDirectory' }),
    );
  });

  it.each<[string, () => Promise<unknown>]>([
    ['an unknown failure', () => Promise.reject(new Error('ipc closed'))],
    ['an unknown error code', () => Promise.reject({ code: 'disk_full', params: {} })],
    ['an unexpected status', async () => [{ ...history[0], status: 'paused' }]],
    ['a missing field', async () => [{ ...history[0], startedAt: undefined }]],
    ['an object instead of a list', async () => ({ items: history })],
  ])('reports %s as an internal error', async (_, respond) => {
    const tauri = backend(respond);

    await expect(firstValueFrom(tauri.listHistory())).rejects.toEqual(new AppError('internal'));
  });

  it('refuses connections with an invalid port or a file engine', async () => {
    for (const connection of [
      { ...connections[0], port: 70000 },
      { ...connections[0], engine: 'sqlite' },
    ]) {
      const tauri = backend(async () => [connection]);

      await expect(firstValueFrom(tauri.listConnections())).rejects.toEqual(
        new AppError('internal'),
      );
      TestBed.resetTestingModule();
    }
  });
});
