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
      sidebar: [
        {
          label: 'Guides',
          translations: { 'zh-CN': '教程' },
          items: [{ autogenerate: { directory: 'guides' } }],
        },
        {
          label: 'Specifications',
          translations: { 'zh-CN': '规范' },
          items: [{ autogenerate: { directory: 'specification' } }],
        },
      ],
    }),
  ],
});
