import { EnvironmentProviders, makeEnvironmentProviders } from '@angular/core';
import { BACKEND } from './backend';
import { MockBackend } from './mock-backend';

export function provideBackend(): EnvironmentProviders {
  return makeEnvironmentProviders([MockBackend, { provide: BACKEND, useExisting: MockBackend }]);
}
