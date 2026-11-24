import { Component } from '@angular/core';
import { TranslocoDirective } from '@jsverse/transloco';
import { StatusBadge } from '../../shared/ui/badge/status-badge';
import { Button } from '../../shared/ui/button/button';
import { Note } from '../../shared/ui/card/note';

/**
 * Agendamentos (A1): prévia desabilitada do que chega na v0.5. Nada é agendado.
 */
@Component({
  selector: 'app-schedules-page',
  imports: [Button, Note, StatusBadge, TranslocoDirective],
  host: { class: 'flex flex-col gap-7' },
  template: `
    <ng-container *transloco="let t; prefix: 'schedules'">
      <div class="flex flex-wrap items-end justify-between gap-4">
        <div class="flex flex-col gap-1.5">
          <app-status-badge tone="warning" class="self-start">{{ t('planned') }}</app-status-badge>
          <h1 class="m-0 text-[32px] leading-[1.1] font-bold tracking-[-0.02em]">
            {{ t('title') }}
          </h1>
        </div>
        <div class="flex flex-col items-end gap-1.5">
          <button
            appButton="primary"
            type="button"
            disabled
            aria-describedby="schedules-unavailable"
          >
            {{ t('new') }}
          </button>
          <span id="schedules-unavailable" class="text-[13px] text-fg-subtle">
            {{ t('available') }}
          </span>
        </div>
      </div>
      <p class="m-0 max-w-[960px] text-[15px] leading-normal text-fg-muted">
        {{ t('description') }}
      </p>
      <section
        class="flex max-w-[960px] flex-wrap items-center gap-6 rounded-[14px] border border-dashed border-border bg-surface p-6"
        [attr.aria-label]="t('preview')"
      >
        <div class="flex flex-[1_1_320px] flex-col gap-1.5">
          <span class="text-lg font-semibold">development · {{ t('fullBackup') }}</span>
          <span class="font-mono text-[13px] text-fg-muted">{{ t('frequency') }}</span>
        </div>
        <label class="flex items-center gap-3 text-[15px] text-fg-subtle">
          <span>{{ t('enabled') }}</span>
          <input
            type="checkbox"
            role="switch"
            checked
            disabled
            aria-describedby="schedules-unavailable"
          />
        </label>
      </section>
      <div class="grid max-w-[960px] grid-cols-[repeat(auto-fit,minmax(280px,1fr))] gap-4">
        <app-note [title]="t('closedApp.title')">{{ t('closedApp.description') }}</app-note>
        <app-note [title]="t('v05.title')">{{ t('v05.description') }}</app-note>
      </div>
    </ng-container>
  `,
})
export class SchedulesPage {}
