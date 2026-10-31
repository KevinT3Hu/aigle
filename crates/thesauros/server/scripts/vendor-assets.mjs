import { copyFileSync } from 'node:fs';
copyFileSync('node_modules/htmx.org/dist/htmx.min.js', 'assets/htmx.min.js');
copyFileSync('node_modules/htmx.org/LICENSE', 'assets/htmx.LICENSE');
