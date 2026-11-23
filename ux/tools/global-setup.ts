import {rmSync} from 'node:fs';
import {join} from 'node:path';

export default function globalSetup(): void {
  rmSync(join(import.meta.dirname, '..', '.out'), {recursive: true, force: true});
}
