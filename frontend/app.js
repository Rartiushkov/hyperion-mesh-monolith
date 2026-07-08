const WS_URL = 'wss://api.hyperion-mesh.example/ws';

const els = {
  status: document.getElementById('status'),
  balance: document.getElementById('balance'),
  btnSimulate: document.getElementById('btn-simulate'),
  btnDeposit: document.getElementById('btn-deposit'),
  log: document.getElementById('log'),
};

let ws = null;
let balance = 1000.00;

function updateBalance(delta) {
  balance += delta;
  els.balance.textContent = balance.toFixed(2);
}

function log(msg, type = 'info') {
  const li = document.createElement('li');
  const time = new Date().toLocaleTimeString();
  li.textContent = `[${time}] ${msg}`;
  li.style.color = type === 'success' ? '#2ecc71' : type === 'error' ? '#ff3b3b' : '#f0f0f0';
  els.log.prepend(li);
}

function connect() {
  els.status.textContent = 'Connecting…';
  try {
    ws = new WebSocket(WS_URL);
  } catch (e) {
    els.status.textContent = 'Offline (demo mode)';
    enableButtons();
    return;
  }

  ws.onopen = () => {
    els.status.textContent = 'Connected to Hyperion node';
    enableButtons();
    log('WebSocket connected');
  };

  ws.onmessage = (ev) => {
    const data = JSON.parse(ev.data);
    log(`Event: ${data.type}`, 'info');
    if (data.balance !== undefined) updateBalance(data.balance - balance);
  };

  ws.onclose = () => {
    els.status.textContent = 'Offline (demo mode)';
    enableButtons();
    log('WebSocket closed', 'error');
  };

  ws.onerror = (err) => {
    els.status.textContent = 'Offline (demo mode)';
    log('WebSocket error', 'error');
  };
}

function enableButtons() {
  els.btnSimulate.disabled = false;
  els.btnDeposit.disabled = false;
}

function send(type, payload) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type, ...payload }));
  } else {
    log('No backend connection, simulating locally', 'info');
  }
}

els.btnSimulate.addEventListener('click', () => {
  send('pay', { amount_eur: 10.0 });
  updateBalance(-10.0);
  log('Payment 10 EUR approved (simulated)', 'success');
});

els.btnDeposit.addEventListener('click', () => {
  send('deposit', { amount_usdt: 100.0 });
  updateBalance(100.0);
  log('Deposit +100 USDT confirmed', 'success');
});

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/service-worker.js').catch(console.error);
}

updateBalance(0);
connect();
