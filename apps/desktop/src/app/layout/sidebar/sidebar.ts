import { Component, inject } from '@angular/core';
import { RouterLink, RouterLinkActive } from '@angular/router';
import { TranslocoDirective } from '@jsverse/transloco';
import { Language, LanguageService } from '../../core/i18n/language.service';
import { Segmented, SegmentedOption } from '../../shared/ui/segmented/segmented';

interface NavItem {
  readonly path: string;
  readonly labelKey: string;
  readonly icon: string;
  readonly plannedFor?: string;
}

const NAV_ITEMS: readonly NavItem[] = [
  {
    path: '/dashboard',
    labelKey: 'nav.dashboard',
    icon: 'M4 4h7v7H4zM13 4h7v4h-7zM13 10h7v10h-7zM4 13h7v7H4z',
  },
  {
    path: '/connections',
    labelKey: 'nav.connections',
    icon: 'M9 7H7a5 5 0 0 0 0 10h2M15 7h2a5 5 0 0 1 0 10h-2M8 12h8',
  },
  {
    path: '/backups',
    labelKey: 'nav.backups',
    icon: 'M5 6c0-1.4 3.1-2.5 7-2.5s7 1.1 7 2.5-3.1 2.5-7 2.5S5 7.4 5 6zM5 6v12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5V6M5 12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5',
  },
  { path: '/restores', labelKey: 'nav.restores', icon: 'M4 12a8 8 0 1 0 2.3-5.7M4 4v4h4' },
  {
    path: '/schedules',
    labelKey: 'nav.schedules',
    icon: 'M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16zM12 8v4l3 2',
    plannedFor: 'v0.5',
  },
  { path: '/history', labelKey: 'nav.history', icon: 'M5 6h14M5 12h14M5 18h9' },
];

const SETTINGS: NavItem = {
  path: '/settings',
  labelKey: 'nav.settings',
  icon: 'M4 7h9M17 7h3M4 17h3M11 17h9M15 5v4M9 15v4',
};

const LINK =
  'flex min-h-11 items-center gap-3 rounded-[10px] px-3 text-[15px] font-normal text-fg-muted no-underline hover:text-fg-strong';
const ACTIVE = 'bg-surface font-semibold text-fg-strong';

/**
 * Barra lateral do desktop: marca, navegação principal, idioma e Configurações no rodapé. O item ativo
 * recebe `aria-current="page"`; Agendamentos mostra o selo da versão em que está planejado.
 */
@Component({
  selector: 'app-sidebar',
  imports: [RouterLink, RouterLinkActive, Segmented, TranslocoDirective],
  host: {
    class:
      'box-border flex h-full flex-col gap-7 border-r border-border-subtle bg-sidebar px-4 py-6 text-fg',
  },
  template: `
    <ng-container *transloco="let t">
      <div class="flex items-center gap-3 px-2">
        <svg viewBox="0 0 512 512" width="36" height="36" aria-hidden="true">
          <rect width="512" height="512" rx="112" fill="#1E1E1E" />
          <path
            d="M168 124 V284 A110 34 0 0 0 388 284 V124 M168 124 A110 34 0 0 1 388 124"
            fill="none"
            stroke="#EDEDED"
            stroke-width="18"
          />
          <defs>
            <mask id="sidebar-bite" maskUnits="userSpaceOnUse" x="0" y="0" width="512" height="512">
              <rect width="512" height="512" fill="#fff" />
              <circle cx="98" cy="224" r="46" fill="#000" />
              <circle cx="94" cy="292" r="42" fill="#000" />
            </mask>
          </defs>
          <g mask="url(#sidebar-bite)">
            <path d="M90 192 V352 A110 34 0 0 0 310 352 V192 Z" fill="#EDEDED" />
            <ellipse
              cx="200"
              cy="192"
              rx="110"
              ry="34"
              fill="#EDEDED"
              stroke="#1E1E1E"
              stroke-width="18"
            />
          </g>
          <circle cx="358" cy="364" r="70" fill="#EDEDED" stroke="#1E1E1E" stroke-width="20" />
        </svg>
        <div class="flex flex-col leading-[1.15]">
          <span class="text-[15px] font-bold">Database Backup</span>
          <span class="text-[13px] text-fg-subtle">Utility</span>
        </div>
      </div>

      <nav [attr.aria-label]="t('nav.label')" class="flex flex-col gap-1">
        @for (item of items; track item.path) {
          <a
            [routerLink]="item.path"
            [class]="link"
            [routerLinkActive]="active"
            ariaCurrentWhenActive="page"
          >
            <svg
              viewBox="0 0 24 24"
              width="20"
              height="20"
              fill="none"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path [attr.d]="item.icon" />
            </svg>
            <span class="flex-auto">{{ t(item.labelKey) }}</span>
            @if (item.plannedFor; as version) {
              <span class="sr-only">({{ t('nav.planned', { version }) }})</span>
              <span
                aria-hidden="true"
                class="rounded-full border border-border px-[7px] py-0.5 font-mono text-[11px] font-medium text-fg-subtle"
                [title]="t('nav.planned', { version })"
                >{{ version }}</span
              >
            }
          </a>
        }
      </nav>

      <div class="mt-auto flex flex-col gap-3">
        <span aria-hidden="true" class="px-1 text-xs text-fg-subtle">{{ t('nav.language') }}</span>
        <app-segmented
          [label]="t('nav.language')"
          [options]="languages"
          [value]="language.language()"
          (valueChange)="language.set($event)"
        />
        <a
          [routerLink]="settings.path"
          [class]="link"
          [routerLinkActive]="active"
          ariaCurrentWhenActive="page"
        >
          <svg
            viewBox="0 0 24 24"
            width="20"
            height="20"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <path [attr.d]="settings.icon" />
          </svg>
          <span>{{ t(settings.labelKey) }}</span>
        </a>
      </div>
    </ng-container>
  `,
})
export class Sidebar {
  protected readonly language = inject(LanguageService);
  protected readonly languages: readonly SegmentedOption<Language>[] = [
    { value: 'pt-BR', label: 'PT-BR' },
    { value: 'en-US', label: 'EN-US' },
  ];
  protected readonly items = NAV_ITEMS;
  protected readonly settings = SETTINGS;
  protected readonly link = LINK;
  protected readonly active = ACTIVE;
}
