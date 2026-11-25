import { Routes } from '@angular/router';

const comingSoon = () =>
  import('./features/coming-soon/coming-soon-page').then((m) => m.ComingSoonPage);

export const routes: Routes = [
  { path: '', pathMatch: 'full', redirectTo: 'dashboard' },
  {
    path: 'dashboard',
    loadComponent: () => import('./features/dashboard/dashboard-page').then((m) => m.DashboardPage),
  },
  { path: 'connections', loadComponent: comingSoon, data: { titleKey: 'nav.connections' } },
  { path: 'backups', loadComponent: comingSoon, data: { titleKey: 'nav.backups' } },
  { path: 'restores', loadComponent: comingSoon, data: { titleKey: 'nav.restores' } },
  {
    path: 'schedules',
    loadComponent: () => import('./features/schedules/schedules-page').then((m) => m.SchedulesPage),
  },
  {
    path: 'history',
    loadComponent: () => import('./features/history/history-page').then((m) => m.HistoryPage),
  },
  {
    path: 'settings',
    loadComponent: () => import('./features/settings/settings-page').then((m) => m.SettingsPage),
  },
  { path: '**', redirectTo: 'dashboard' },
];
