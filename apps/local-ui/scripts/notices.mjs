import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const root = resolve(import.meta.dirname, '..');
const lock = JSON.parse(readFileSync(resolve(root, 'package-lock.json'), 'utf8'));
let text = 'Topicairn embedded UI — third-party notices\n\n';
for (const [path, entry] of Object.entries(lock.packages).sort(([a], [b]) =>
  a.localeCompare(b, 'en'),
)) {
  if (!path || entry.dev || path.startsWith('node_modules/@types/')) continue;
  const pkg = JSON.parse(readFileSync(resolve(root, path, 'package.json'), 'utf8'));
  text += `\n${'='.repeat(72)}\n${pkg.name} ${pkg.version} — ${pkg.license || 'see package license'}\n${pkg.repository?.url || pkg.homepage || ''}\n\n`;
  const licenses = readdirSync(resolve(root, path))
    .filter((name) => /^(license|licence|copying|notice)(\.|$|-)/i.test(name))
    .sort();
  for (const file of licenses)
    text += `${file}\n${readFileSync(resolve(root, path, file), 'utf8')}\n`;
}
writeFileSync(resolve(root, '../../server/domain/ui/third-party-ui.txt'), text);
