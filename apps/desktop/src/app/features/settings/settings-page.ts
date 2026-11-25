import { Component, DestroyRef, inject, signal } from '@angular/core';
import { rxResource, takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { TranslocoDirective } from '@jsverse/transloco';
import { BACKEND } from '../../core/backend/backend';
import { Settings, ToolRunner } from '../../core/backend/backend.models';
import { Language, LanguageService } from '../../core/i18n/language.service';
import { Theme, ThemeService } from '../../core/theme/theme.service';
import { ErrorAlert } from '../../shared/ui/alert/error-alert';
import { Button } from '../../shared/ui/button/button';
import { Control } from '../../shared/ui/field/control';
import { Field } from '../../shared/ui/field/field';
import { PageHeader } from '../../shared/ui/page-header/page-header';
import { Segmented, SegmentedOption } from '../../shared/ui/segmented/segmented';

type Translate = (key: string) => string;

/**
 * Configurações (S1): idioma e tema, que valem na hora, e executor e pasta padrão, gravados pelo
 * backend.
 */
@Component({
  selector: 'app-settings-page',
  imports: [Button, Control, ErrorAlert, Field, PageHeader, Segmented, TranslocoDirective],
  host: { class: 'flex flex-col gap-7' },
  templateUrl: './settings-page.html',
})
export class SettingsPage {
  private readonly backend = inject(BACKEND);
  private readonly destroyRef = inject(DestroyRef);
  protected readonly languageService = inject(LanguageService);
  protected readonly themeService = inject(ThemeService);

  protected readonly settings = rxResource({ stream: () => this.backend.getSettings() });
  protected readonly saveError = signal<unknown>(null);

  protected readonly languages: readonly SegmentedOption<Language>[] = [
    { value: 'pt-BR', label: 'PT-BR' },
    { value: 'en-US', label: 'EN-US' },
  ];

  protected themes(t: Translate): readonly SegmentedOption<Theme>[] {
    return [
      { value: 'dark', label: t('dark') },
      { value: 'light', label: t('light') },
    ];
  }

  protected runners(t: Translate): readonly SegmentedOption<ToolRunner>[] {
    return [
      { value: 'native', label: t('native') },
      { value: 'docker', label: t('docker') },
    ];
  }

  protected save(current: Settings, change: Partial<Settings>): void {
    this.saveError.set(null);
    const next = { ...current, ...change };
    this.settings.set(next);
    this.backend
      .saveSettings(next)
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: (saved) => this.settings.set(saved),
        error: (error: unknown) => {
          this.saveError.set(error);
          this.settings.set(current);
        },
      });
  }
}
