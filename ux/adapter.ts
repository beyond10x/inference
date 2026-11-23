export type Viewport = {name: string; width: number; height: number};
export type Theme = 'light' | 'dark';

export type AlwaysVisible = {
  name: string;
  selector: string;
  minWidth?: number;
  docsOnly?: boolean;
};

export const adapter = {
  site: 'inference documentation',
  basePath: '/inference/',
  port: 4317,
  gatingViewports: [
    {name: '1440x900', width: 1440, height: 900},
    {name: '390x844', width: 390, height: 844},
  ] as Viewport[],
  measuredViewports: [
    {name: '1280x800', width: 1280, height: 800},
    {name: '1024x768', width: 1024, height: 768},
    {name: '768x1024', width: 768, height: 1024},
  ] as Viewport[],
  themes: ['light', 'dark'] as Theme[],
  alwaysVisible: [
    {name: 'navbar', selector: 'nav.navbar'},
    {name: 'search', selector: '.navbar .navbar__search, .navbar [role="search"], .navbar .DocSearch-Button'},
    {name: 'theme toggle', selector: '.navbar button[aria-label*="dark and light mode"]'},
    {name: 'sidebar', selector: 'aside.theme-doc-sidebar-container nav.menu', minWidth: 997, docsOnly: true},
  ] as AlwaysVisible[],
  primaryNavigation: 'nav.navbar, aside.theme-doc-sidebar-container, .navbar-sidebar',
  menuButton: {role: 'button' as const, name: /navigation bar/i},
  axeTags: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'],
  keyboardMaxTabs: 40,
};

export function url(path: string): string {
  return `http://127.0.0.1:${adapter.port}${adapter.basePath}${path.replace(/^\//, '')}`;
}
