import { Component, input } from '@angular/core';
import { TranslocoDirective } from '@jsverse/transloco';
import { Note } from '../../shared/ui/card/note';
import { PageHeader } from '../../shared/ui/page-header/page-header';

/**
 * Página das telas que ainda não existem. O título vem de `titleKey`, nos dados da rota.
 */
@Component({
  selector: 'app-coming-soon-page',
  imports: [Note, PageHeader, TranslocoDirective],
  host: { class: 'flex flex-col gap-7' },
  template: `
    <ng-container *transloco="let t">
      <app-page-header [title]="t(titleKey())" />
      <app-note>{{ t('comingSoon.description') }}</app-note>
    </ng-container>
  `,
})
export class ComingSoonPage {
  readonly titleKey = input.required<string>();
}
