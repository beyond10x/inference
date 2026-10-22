import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    {
      type: 'category',
      label: 'Start here',
      collapsed: false,
      items: ['index', 'getting-started', 'status'],
    },
    {
      type: 'category',
      label: 'Concepts',
      collapsed: false,
      items: [
        'concepts/overview',
        'concepts/contracts',
        'concepts/streams',
        'concepts/graphs',
        'concepts/runtime',
        'concepts/session',
        'concepts/deployment-profiles',
        'concepts/usage',
      ],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: [
        'guides/talk-to-a-hosted-voice-model',
        'guides/run-the-local-voice-cascade-offline',
        'guides/swap-the-recogniser-by-configuration',
        'guides/transcribe-a-wav-file',
        'guides/compare-voice-providers',
        'guides/estimate-cost-from-a-price-table',
        'guides/run-the-checks',
      ],
    },
    {
      type: 'category',
      label: 'Examples',
      collapsed: false,
      items: ['examples/index', 'examples/local-voice-cascade', 'examples/repo-podcast'],
    },
    {
      type: 'category',
      label: 'Reference',
      collapsed: false,
      items: ['reference/crates', 'reference/adapters', 'reference/errors', 'reference/environment'],
    },
    {
      type: 'category',
      label: 'Project status',
      collapsed: false,
      items: ['status/limitations'],
    },
  ],
};

export default sidebars;
