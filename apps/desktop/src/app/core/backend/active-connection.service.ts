import { Injectable, computed, inject } from '@angular/core';
import { rxResource } from '@angular/core/rxjs-interop';
import { BACKEND } from './backend';

@Injectable({ providedIn: 'root' })
export class ActiveConnectionService {
  private readonly backend = inject(BACKEND);
  private readonly connections = rxResource({ stream: () => this.backend.listConnections() });

  readonly name = computed(() =>
    this.connections.hasValue() ? (this.connections.value()[0]?.name ?? null) : null,
  );
}
