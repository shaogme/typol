import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://shaogme.github.io',
  base: '/typol',
  integrations: [
    starlight({
      title: {
        en: 'Typol Documentation',
        'zh-CN': 'Typol 文档',
      },
      defaultLocale: 'root',
      locales: {
        root: {
          label: 'English',
          lang: 'en',
        },
        'zh-cn': {
          label: '简体中文',
          lang: 'zh-CN',
        },
      },
      customCss: ['/src/styles/typol.css'],
      sidebar: [
        {
          label: 'Guides',
          translations: { 'zh-CN': '教程' },
          items: [
            'guides/quickstart',
            'guides/cli',
            'guides/advanced',
            'guides/no_std',
            'guides/nix',
          ],
        },
        {
          label: 'Specifications',
          translations: { 'zh-CN': '规范' },
          items: [
            {
              label: 'Schema',
              translations: { 'zh-CN': 'Schema' },
              items: ['specification/schema-dsl'],
            },
            {
              label: 'Config',
              translations: { 'zh-CN': 'Config' },
              items: ['specification/config-dsl', 'specification/config-ir'],
            },
            {
              label: 'Implementation details',
              translations: { 'zh-CN': '实现细节' },
              items: [
                'specification/schema-implementation',
                'specification/config-implementation',
                'specification/regex-implementation',
              ],
            },
          ],
        },
      ],
    }),
  ],
});
