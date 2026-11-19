import { Component } from '@angular/core';
import { PageHeader } from '../../shared/ui/page-header/page-header';

/**
 * Painel (P1). Por enquanto só o cabeçalho; os cards e a atividade recente entram no DB-51.
 */
@Component({
  selector: 'app-dashboard-page',
  imports: [PageHeader],
  template: '<app-page-header overline="Visão geral" title="Painel" />',
})
export class DashboardPage {}
