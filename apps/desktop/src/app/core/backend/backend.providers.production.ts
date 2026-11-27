import { EnvironmentProviders, makeEnvironmentProviders } from '@angular/core';
import { TauriBackend } from '../tauri/tauri-backend';
import { BACKEND } from './backend';

export function provideBackend(): EnvironmentProviders {
  return makeEnvironmentProviders([TauriBackend, { provide: BACKEND, useExisting: TauriBackend }]);
}
