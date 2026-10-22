import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import {withProductSite} from '@beyond10x/docs-system/product-site';

const config: Config = {
  title: 'inference',
  tagline: 'Swap the model. Keep the product.',
  favicon: 'img/mark.svg',

  future: {
    v4: true,
  },

  url: 'https://beyond10x.github.io',
  baseUrl: '/inference/',

  organizationName: 'beyond10x',
  projectName: 'inference',
  deploymentBranch: 'gh-pages',
  trailingSlash: false,

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',

  markdown: {
    format: 'detect',
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
    mermaid: true,
  },

  themes: ['@docusaurus/theme-mermaid'],

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          routeBasePath: 'docs',
          editUrl: 'https://github.com/beyond10x/inference/tree/main/website/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    navbar: {
      title: 'inference',
      items: [
        {to: '/docs/', label: 'Documentation', position: 'left'},
        {to: '/docs/getting-started', label: 'Getting started', position: 'left'},
        {to: '/docs/reference/crates', label: 'Crates', position: 'left'},
        {to: '/docs/status', label: 'Status', position: 'left'},
        {href: 'https://github.com/beyond10x/inference', label: 'GitHub', position: 'right'},
      ],
    },
    footer: {
      links: [
        {
          title: 'Documentation',
          items: [
            {label: 'Overview', to: '/docs/'},
            {label: 'Getting started', to: '/docs/getting-started'},
            {label: 'Concepts', to: '/docs/concepts/overview'},
            {label: 'Crates', to: '/docs/reference/crates'},
            {label: 'Adapters', to: '/docs/reference/adapters'},
            {label: 'Status', to: '/docs/status'},
          ],
        },
        {
          title: 'Guides',
          items: [
            {label: 'Talk to a hosted voice model', to: '/docs/guides/talk-to-a-hosted-voice-model'},
            {label: 'Run the voice cascade offline', to: '/docs/guides/run-the-local-voice-cascade-offline'},
            {label: 'Compare voice providers', to: '/docs/guides/compare-voice-providers'},
            {label: 'Run the checks', to: '/docs/guides/run-the-checks'},
          ],
        },
        {
          title: 'Project',
          items: [
            {label: 'Source', href: 'https://github.com/beyond10x/inference'},
            {label: 'Releases', href: 'https://github.com/beyond10x/inference/releases'},
            {label: 'Changelog', href: 'https://github.com/beyond10x/inference/blob/main/CHANGELOG.md'},
          ],
        },
      ],
      copyright: 'A beyond10x project. inference · Apache-2.0.',
    },
  } satisfies Preset.ThemeConfig,
};

export default withProductSite(config, {landing: './product.json', mark: 'IN'});
