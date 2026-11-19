import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Alert } from './alert/alert';
import { StatusBadge } from './badge/status-badge';
import { Dialog } from './dialog/dialog';
import { FilePicker } from './file-picker/file-picker';
import { Progress } from './progress/progress';

@Component({
  imports: [Alert, StatusBadge, Dialog, FilePicker, Progress],
  template: `
    <app-status-badge tone="success">Verificado</app-status-badge>
    <app-alert tone="danger" title="Falha na autenticação" code="auth_failed" codeLabel="erro">
      O servidor recusou as credenciais.
    </app-alert>
    <app-alert tone="warning" title="Ação destrutiva">Exige confirmação explícita.</app-alert>
    <app-progress label="Dump em streaming" [value]="progress()" [valueText]="progressText()" />
    <app-file-picker
      label="Arquivo do banco"
      [path]="path()"
      emptyLabel="Nenhum arquivo escolhido"
      chooseLabel="Escolher arquivo…"
      changeLabel="Trocar…"
      (pick)="picks.set(picks() + 1)"
    />
    <app-dialog [(open)]="open" title="Chaves age" description="A chave pública criptografa.">
      <p>Corpo</p>
      <button dialog-actions type="button">Fechar</button>
    </app-dialog>
  `,
})
class Host {
  readonly progress = signal<number | null>(null);
  readonly progressText = signal('1,24 GB gravados');
  readonly path = signal<string | null>(null);
  readonly picks = signal(0);
  readonly open = signal(false);
}

describe('shared ui', () => {
  beforeAll(() => {
    const prototype = HTMLDialogElement.prototype;
    if (typeof prototype.showModal !== 'function') {
      prototype.showModal = function (this: HTMLDialogElement) {
        this.setAttribute('open', '');
      };
      prototype.close = function (this: HTMLDialogElement) {
        this.removeAttribute('open');
        this.dispatchEvent(new Event('close'));
      };
    }
  });

  async function render() {
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    return { fixture, element: fixture.nativeElement as HTMLElement };
  }

  it('hides the decorative status symbol from screen readers', async () => {
    const { element } = await render();
    const badge = element.querySelector('app-status-badge') as HTMLElement;

    expect(badge.textContent?.replace(/\s+/g, '')).toBe('✓Verificado');
    expect(badge.querySelector('[aria-hidden="true"]')?.textContent).toBe('✓');
  });

  it('announces errors with the stable code and keeps warnings quiet', async () => {
    const { element } = await render();
    const [error, warning] = [...element.querySelectorAll('app-alert')];

    expect(error.getAttribute('role')).toBe('alert');
    expect(error.textContent).toContain('erro: auth_failed');
    expect(warning.getAttribute('role')).toBeNull();
  });

  it('reports indeterminate progress as text without a numeric value', async () => {
    const { element } = await render();
    const bar = element.querySelector('[role="progressbar"]') as HTMLElement;

    expect(bar.getAttribute('aria-valuenow')).toBeNull();
    expect(bar.getAttribute('aria-valuetext')).toBe('1,24 GB gravados');
    expect(bar.querySelector('.animate-indeterminate')).not.toBeNull();
  });

  it('reports determinate progress clamped between 0 and 100', async () => {
    const { fixture, element } = await render();
    fixture.componentInstance.progress.set(140);
    fixture.componentInstance.progressText.set('100%');
    await fixture.whenStable();
    const bar = element.querySelector('[role="progressbar"]') as HTMLElement;

    expect(bar.getAttribute('aria-valuenow')).toBe('100');
    expect(bar.getAttribute('aria-valuemax')).toBe('100');
    expect((bar.firstElementChild as HTMLElement).style.width).toBe('100%');
  });

  it('asks for a file and switches the button text once a path is chosen', async () => {
    const { fixture, element } = await render();
    const picker = element.querySelector('app-file-picker') as HTMLElement;
    const button = picker.querySelector('button') as HTMLButtonElement;

    expect(picker.textContent).toContain('Nenhum arquivo escolhido');
    button.click();
    expect(fixture.componentInstance.picks()).toBe(1);

    fixture.componentInstance.path.set('~/dados/app.sqlite3');
    await fixture.whenStable();

    expect(picker.textContent).toContain('~/dados/app.sqlite3');
    expect(button.textContent?.trim()).toBe('Trocar…');
    expect(button.getAttribute('aria-label')).toBe('Trocar… Arquivo do banco');
  });

  it('opens and closes the native dialog from the bound state', async () => {
    const { fixture, element } = await render();
    const dialog = element.querySelector('dialog') as HTMLDialogElement;

    expect(dialog.open).toBe(false);
    fixture.componentInstance.open.set(true);
    await fixture.whenStable();

    expect(dialog.open).toBe(true);
    expect(element.querySelector(`#${dialog.getAttribute('aria-labelledby')}`)?.textContent).toBe(
      'Chaves age',
    );

    dialog.close();
    await fixture.whenStable();

    expect(fixture.componentInstance.open()).toBe(false);
  });
});
