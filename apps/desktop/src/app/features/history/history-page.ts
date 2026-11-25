import { Component, computed, inject, signal } from '@angular/core';
import { rxResource } from '@angular/core/rxjs-interop';
import { TranslocoDirective } from '@jsverse/transloco';
import { BACKEND } from '../../core/backend/backend';
import { ENGINE_LABELS, OperationKind } from '../../core/backend/backend.models';
import { OperationStatusBadge } from '../../shared/operations/operation-status';
import { DateTimePipe } from '../../shared/pipes/format.pipes';
import { Alert } from '../../shared/ui/alert/alert';
import { Tag } from '../../shared/ui/badge/tag';
import { Button } from '../../shared/ui/button/button';
import { Note } from '../../shared/ui/card/note';
import { PageHeader } from '../../shared/ui/page-header/page-header';
import { Segmented, SegmentedOption } from '../../shared/ui/segmented/segmented';

type HistoryFilter = 'all' | OperationKind;
type Translate = (key: string) => string;

/**
 * Histórico (H1): backups e restaurações de todas as conexões, com filtro por tipo. As linhas
 * passam a abrir o detalhe quando as telas B4, B5 e R4 existirem.
 */
@Component({
  selector: 'app-history-page',
  imports: [
    Alert,
    Button,
    DateTimePipe,
    Note,
    OperationStatusBadge,
    PageHeader,
    Segmented,
    Tag,
    TranslocoDirective,
  ],
  host: { class: 'flex flex-col gap-7' },
  templateUrl: './history-page.html',
})
export class HistoryPage {
  private readonly backend = inject(BACKEND);

  protected readonly engines = ENGINE_LABELS;
  protected readonly filter = signal<HistoryFilter>('all');
  protected readonly history = rxResource({ stream: () => this.backend.listHistory() });

  protected readonly rows = computed(() => {
    const filter = this.filter();
    const items = this.history.hasValue() ? this.history.value() : [];
    return filter === 'all' ? items : items.filter(({ kind }) => kind === filter);
  });

  protected options(t: Translate): readonly SegmentedOption<HistoryFilter>[] {
    return [
      { value: 'all', label: t('history.filters.all') },
      { value: 'backup', label: t('history.filters.backup') },
      { value: 'restore', label: t('history.filters.restore') },
    ];
  }
}
