import { EnvironmentProviders, makeEnvironmentProviders } from '@angular/core';
import { isTauri } from '@tauri-apps/api/core';
import { TauriBackend } from '../tauri/tauri-backend';
import { BACKEND } from './backend';
import { MockBackend } from './mock-backend';

export function provideBackend(): EnvironmentProviders {
  if (isTauri()) {
    return makeEnvironmentProviders([
      TauriBackend,
      { provide: BACKEND, useExisting: TauriBackend },
    ]);
  }
  return makeEnvironmentProviders([MockBackend, { provide: BACKEND, useExisting: MockBackend }]);
}
