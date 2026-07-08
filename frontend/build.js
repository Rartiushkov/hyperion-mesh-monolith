const fs = require('fs');
const path = require('path');

const files = ['index.html', 'app.js', 'style.css', 'manifest.json', 'service-worker.js'];
const dist = path.join(__dirname, 'dist');

fs.mkdirSync(dist, { recursive: true });
for (const f of files) {
  fs.copyFileSync(path.join(__dirname, f), path.join(dist, f));
}

console.log('Hyperion Mesh frontend built into frontend/dist/');
