const fs = require('fs');
const path = require('path');
const https = require('https');
const crypto = require('crypto');
const os = require('os');
const { execSync } = require('child_process');

const VERSION = "0.8.51"; // Must match package.json
const OWNER = 'mblua';
const REPO = 'AgentsCommander';

const platform = os.platform();
const arch = os.arch();

// Map Node platform/arch to our release asset names
let assetName = '';
if (platform === 'darwin') {
  assetName = `agentscommander-mac-${arch === 'arm64' ? 'aarch64' : 'x86_64'}.app.tar.gz`;
} else if (platform === 'win32') {
  assetName = `agentscommander-windows-${arch === 'arm64' ? 'aarch64' : 'x86_64'}.exe`;
} else if (platform === 'linux') {
  assetName = `agentscommander-linux-${arch === 'arm64' ? 'aarch64' : 'x86_64'}`;
} else {
  console.error(`Unsupported platform: ${platform}`);
  process.exit(1);
}

const binDir = path.join(__dirname, 'bin');
if (!fs.existsSync(binDir)) {
  fs.mkdirSync(binDir, { recursive: true });
}

const targetPath = path.join(binDir, assetName);
const tmpPath = targetPath + '.tmp';
const shasumUrl = `https://github.com/${OWNER}/${REPO}/releases/download/v${VERSION}/SHASUMS256.txt`;
const assetUrl = `https://github.com/${OWNER}/${REPO}/releases/download/v${VERSION}/${assetName}`;

function fetchUrl(url, redirectCount = 0) {
  return new Promise((resolve, reject) => {
    if (redirectCount > 5) {
      return reject(new Error('Too many redirects'));
    }
    https.get(url, (res) => {
      if (res.statusCode === 301 || res.statusCode === 302) {
        res.resume();
        return fetchUrl(res.headers.location, redirectCount + 1).then(resolve).catch(reject);
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`Failed to fetch ${url}: HTTP ${res.statusCode}`));
      }
      resolve(res);
    }).on('error', reject);
  });
}

function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    fetchUrl(url).then(res => {
      const file = fs.createWriteStream(dest);
      
      const handleError = (err) => {
        file.close(() => {
          if (fs.existsSync(dest)) fs.unlinkSync(dest);
          reject(err);
        });
      };

      res.on('error', handleError);
      file.on('error', handleError);

      res.pipe(file);
      file.on('finish', () => {
        file.close(() => {
          resolve();
        });
      });
    }).catch(reject);
  });
}

async function verifyChecksum(filePath, fileName, expectedShasums) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256');
    const stream = fs.createReadStream(filePath);
    stream.on('data', data => hash.update(data));
    stream.on('end', () => {
      const fileHash = hash.digest('hex');
      const expectedLines = expectedShasums.split('\n');
      const expectedLine = expectedLines.find(line => line.includes(fileName));
      if (!expectedLine) {
        return reject(new Error(`Checksum for ${fileName} not found in SHASUMS256.txt`));
      }
      const expectedHash = expectedLine.split(/\s+/)[0];
      if (fileHash !== expectedHash) {
        return reject(new Error(`Checksum mismatch for ${fileName}. Expected: ${expectedHash}, Actual: ${fileHash}`));
      }
      resolve();
    });
    stream.on('error', reject);
  });
}

async function main() {
  const shasumTmpPath = path.join(binDir, 'SHASUMS256.txt.tmp');
  try {
    console.log(`Downloading SHASUMS256.txt from ${shasumUrl}`);
    // moved to top
    await downloadFile(shasumUrl, shasumTmpPath);
    const expectedShasums = fs.readFileSync(shasumTmpPath, 'utf8');

    console.log(`Downloading ${assetName} from ${assetUrl}`);
    await downloadFile(assetUrl, tmpPath);

    console.log(`Verifying checksum for ${assetName}`);
    await verifyChecksum(tmpPath, assetName, expectedShasums);
    console.log('Checksum verified.');

    if (platform === 'darwin') {
      console.log('Extracting macOS bundle...');
      const extractTmpDir = path.join(binDir, 'tmp-extract');
      if (fs.existsSync(extractTmpDir)) fs.rmSync(extractTmpDir, { recursive: true, force: true });
      fs.mkdirSync(extractTmpDir, { recursive: true });
      try {
        execSync(`tar -xzf "${tmpPath}" -C "${extractTmpDir}"`);
        const extractedFiles = fs.readdirSync(extractTmpDir);
        for (const file of extractedFiles) {
          const dest = path.join(binDir, file);
          if (fs.existsSync(dest)) fs.rmSync(dest, { recursive: true, force: true });
          fs.renameSync(path.join(extractTmpDir, file), dest);
        }
      } finally {
        if (fs.existsSync(extractTmpDir)) fs.rmSync(extractTmpDir, { recursive: true, force: true });
      }
      fs.unlinkSync(tmpPath);
    } else {
      const finalBinPath = path.join(binDir, platform === 'win32' ? 'agentscommander.exe' : 'agentscommander');
      if (platform !== 'win32') {
        fs.chmodSync(tmpPath, 0o755);
      }
      fs.renameSync(tmpPath, finalBinPath);
    }
    
    fs.unlinkSync(shasumTmpPath);
    console.log('Installation completed successfully.');
  } catch (err) {
    console.error('Installation failed:', err.message);
    if (fs.existsSync(tmpPath)) fs.unlinkSync(tmpPath);
    if (typeof shasumTmpPath !== 'undefined' && fs.existsSync(shasumTmpPath)) fs.unlinkSync(shasumTmpPath);
    process.exit(1);
  }
}

main();
