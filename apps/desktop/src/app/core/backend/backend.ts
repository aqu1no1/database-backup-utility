import { InjectionToken } from '@angular/core';
import { Observable } from 'rxjs';
import { Connection, HistoryItem, Settings } from './backend.models';

export interface Backend {
  listConnections(): Observable<readonly Connection[]>;
  listHistory(): Observable<readonly HistoryItem[]>;
  getSettings(): Observable<Settings>;
  saveSettings(settings: Settings): Observable<Settings>;
}

export const BACKEND = new InjectionToken<Backend>('Backend');
