import { Component, computed, inject } from '@angular/core';
import { rxResource } from '@angular/core/rxjs-interop';
import { RouterLink } from '@angular/router';
import { TranslocoDirective } from '@jsverse/transloco';
import { forkJoin } from 'rxjs';
import { BACKEND } from '../../core/backend/backend';
import { DATABASE_ENGINES, ENGINE_LABELS } from '../../core/backend/backend.models';
import { OperationStatusBadge } from '../../shared/operations/operation-status';
import { DateTimePipe } from '../../shared/pipes/format.pipes';
import { Alert } from '../../shared/ui/alert/alert';
import { Button } from '../../shared/ui/button/button';
import { MetricCard } from '../../shared/ui/card/metric-card';
import { PageHeader } from '../../shared/ui/page-header/page-header';

const RECENT_LIMIT = 3;

/**
 * Painel (P1): último backup, backups em execução, conexões por SGBD, falhas e atividade recente.
 */
@Component({
  selector: 'app-dashboard-page',
  imports: [
    Alert,
    Button,
    DateTimePipe,
    MetricCard,
    OperationStatusBadge,
    PageHeader,
    RouterLink,
    TranslocoDirective,
  ],
  host: { class: 'flex flex-col gap-7' },
  templateUrl: './dashboard-page.html',
})
export class DashboardPage {
  private readonly backend = inject(BACKEND);

  protected readonly engines = ENGINE_LABELS;
  protected readonly overview = rxResource({
    stream: () =>
      forkJoin({
        connections: this.backend.listConnections(),
        history: this.backend.listHistory(),
      }),
  });

  protected readonly summary = computed(() => {
    if (!this.overview.hasValue()) {
      return null;
    }
    const { connections, history } = this.overview.value();
    const items = [...history].sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt));
    const backups = items.filter(({ kind }) => kind === 'backup');
    const running = backups.filter(({ status }) => status === 'running');
    return {
      lastBackup: backups.find(({ status }) => status !== 'running') ?? null,
      running: running.length,
      runningNames: running.map(({ source }) => source).join(', '),
      connections: connections.length,
      byEngine: DATABASE_ENGINES.map((engine) => ({
        label: ENGINE_LABELS[engine],
        count: connections.filter((connection) => connection.engine === engine).length,
      }))
        .filter(({ count }) => count > 0)
        .map(({ label, count }) => `${label} ${count}`)
        .join(' · '),
      failures: items.filter(({ status }) => status === 'failed').length,
      recent: items.slice(0, RECENT_LIMIT),
    };
  });
}
