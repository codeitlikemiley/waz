# Waz Website

Astro site, derived from the mockup under `design/`.
```bash
npm install
npm run dev      # http://localhost:4321
npm run build    # outputs dist/
```

structure:
- `src/pages/index.astro` — Landing
- `src/pages/docs/[...slug].astro` — dynamic routing of documents- `src/content/docs/*.mdx` — Document content (Content Collections)- `src/components/` — Nav / Footer / Banner etc.- `src/styles/` — design tokens and global styles
