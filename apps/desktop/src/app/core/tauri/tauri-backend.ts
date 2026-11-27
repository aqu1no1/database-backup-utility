import { Injectable, InjectionToken, inject } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { Observable, catchError, defer, from, map, throwError } from 'rxjs';
import { Backend } from '../backend/backend';
import { Connection, HistoryItem, Settings } from '../backend/backend.models';
import { parseConnections, parseHistory, parseSettings, toAppError } from './tauri-contract';

export type TauriInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

export const TAURI_INVOKE = new InjectionToken<TauriInvoke>('TauriInvoke', {
  providedIn: 'root',
  factory: () => (command, args) => invoke(command, args),
});

@Injectable()
export class TauriBackend implements Backend {
  private readonly invoke = inject(TAURI_INVOKE);

  listConnections(): Observable<readonly Connection[]> {
    return this.call('connections_list', parseConnections);
  }

  listHistory(): Observable<readonly HistoryItem[]> {
    return this.call('history_list', parseHistory);
  }

  getSettings(): Observable<Settings> {
    return this.call('settings_get', parseSettings);
  }

  saveSettings(settings: Settings): Observable<Settings> {
    return this.call('settings_set', parseSettings, { settings });
  }

  private call<T>(
    command: string,
    parse: (value: unknown) => T,
    args?: Record<string, unknown>,
  ): Observable<T> {
    return defer(() => from(this.invoke(command, args))).pipe(
      map(parse),
      catchError((error: unknown) => throwError(() => toAppError(error))),
    );
  }
}
