import { Injectable } from '@angular/core';
import { Observable, defer, of, switchMap, throwError, timer } from 'rxjs';
import { Backend } from './backend';
import { AppError, Connection, ErrorCode, HistoryItem, Settings } from './backend.models';
import { MOCK_CONNECTIONS, MOCK_HISTORY, MOCK_SETTINGS } from './mock-data';

const LATENCY_MS = 250;

@Injectable()
export class MockBackend implements Backend {
  latencyMs = LATENCY_MS;
  connections: readonly Connection[] = MOCK_CONNECTIONS;
  history: readonly HistoryItem[] = MOCK_HISTORY;

  private settings = MOCK_SETTINGS;
  private readonly failures = new Map<keyof Backend, ErrorCode>();

  failWith(method: keyof Backend, code: ErrorCode): void {
    this.failures.set(method, code);
  }

  recover(method: keyof Backend): void {
    this.failures.delete(method);
  }

  listConnections(): Observable<readonly Connection[]> {
    return this.respond('listConnections', () => this.connections);
  }

  listHistory(): Observable<readonly HistoryItem[]> {
    return this.respond('listHistory', () => this.history);
  }

  getSettings(): Observable<Settings> {
    return this.respond('getSettings', () => this.settings);
  }

  saveSettings(settings: Settings): Observable<Settings> {
    return this.respond('saveSettings', () => (this.settings = settings));
  }

  private respond<T>(method: keyof Backend, produce: () => T): Observable<T> {
    return defer(() => {
      const code = this.failures.get(method);
      const response = code ? throwError(() => new AppError(code)) : of(produce());
      return this.latencyMs > 0 ? timer(this.latencyMs).pipe(switchMap(() => response)) : response;
    });
  }
}
