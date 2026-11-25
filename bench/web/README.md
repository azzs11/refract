# Web benchmark

```sh
cd web && npm run wasm && npm run build
BENCH=1 npx playwright test tests/bench.spec.ts          # -> bench/results/web.json
npm run preview &                                         # http://localhost:4173
npx lighthouse@12.6.1 http://localhost:4173/ --chrome-flags="--headless=new" \
  --only-categories=performance,accessibility,best-practices,seo --output=json
```

`bench/results/web.md` summarises both (scores in `bench/results/lighthouse.json`).
