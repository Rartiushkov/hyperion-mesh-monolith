const els = {
  log: document.getElementById("log"),
  authForm: document.getElementById("auth-form"),
  kycForm: document.getElementById("kyc-form"),
  sharedKycForm: document.getElementById("shared-kyc-form"),
  tbankForm: document.getElementById("tbank-form"),
  internetForm: document.getElementById("internet-form"),
  permanentForm: document.getElementById("permanent-form"),
  txCount: document.getElementById("tx-count"),
  kycLevel: document.getElementById("kyc-level"),
  gatewayStatus: document.getElementById("gateway-status"),
  railStatus: document.getElementById("rail-status"),
  workspacePreview: document.getElementById("workspace-preview"),
};

function writeLog(message, tone = "info") {
  const item = document.createElement("li");
  item.className = `log-item ${tone}`;
  item.textContent = `[${new Date().toLocaleTimeString()}] ${message}`;
  els.log.prepend(item);
}

function setGatewayStatus(text, tone = "ready") {
  els.gatewayStatus.textContent = text;
  els.gatewayStatus.dataset.tone = tone;
}

function updateState(data = {}) {
  if (typeof data.tx_count === "number") {
    els.txCount.textContent = String(data.tx_count);
  }
  if (typeof data.kyc_level === "string" && data.kyc_level) {
    els.kycLevel.textContent = data.kyc_level;
  }
}

function setRailStatus(text) {
  els.railStatus.textContent = text;
}

function prettyJson(value) {
  return JSON.stringify(value, null, 2);
}

function renderPreview(html) {
  els.workspacePreview.innerHTML = html;
}

function renderPaymentPreview(data) {
  const paymentLink = data.payment_url
    ? `<a href="${data.payment_url}" target="_blank" rel="noreferrer">Open payment page</a>`
    : "No hosted payment URL returned.";
  const qrLink = data.qr_data
    ? `<a href="${data.qr_data}" target="_blank" rel="noreferrer">Open SBP QR payload</a>`
    : "No SBP QR payload for this rail.";

  renderPreview(`
    <div class="preview-card">
      <h4>Funding session created</h4>
      <p>Payment ID: ${data.payment_id || "pending"} · Status: ${data.status || "unknown"}</p>
      <p>${paymentLink}</p>
      <p>${qrLink}</p>
      <pre>${prettyJson(data.provider_response || {})}</pre>
    </div>
  `);
}

function renderCodegoIframe(data) {
  renderPreview(`
    <div class="preview-card">
      <h4>Hosted Codego KYC session</h4>
      <p>Session ${data.session_id || "pending"} expires at ${data.expires_at || "unknown"}.</p>
      <iframe src="${data.iframe_url}" title="Codego KYC" allow="camera"></iframe>
    </div>
  `);
}

function renderProviderPreview(title, subtitle, payload) {
  renderPreview(`
    <div class="preview-card">
      <h4>${title}</h4>
      <p>${subtitle}</p>
      <pre>${prettyJson(payload || {})}</pre>
    </div>
  `);
}

async function postJson(path, payload) {
  setGatewayStatus("sending", "busy");
  const response = await fetch(path, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
    },
    body: JSON.stringify(payload),
  });

  const text = await response.text();
  let data = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }

  if (!response.ok) {
    setGatewayStatus("error", "error");
    throw new Error(data.error || data.raw || `HTTP ${response.status}`);
  }

  setGatewayStatus("ready", "ready");
  return data;
}

els.tbankForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const paymentMethod = document.getElementById("tbank-payment-method").value;
  const payload = {
    amount_minor: Number(document.getElementById("tbank-amount-minor").value),
    order_id: document.getElementById("tbank-order-id").value.trim(),
    description: document.getElementById("tbank-description").value.trim(),
    customer_key: document.getElementById("tbank-customer-key").value.trim(),
    payment_method: paymentMethod,
    pay_type: paymentMethod === "SBP" ? "O" : undefined,
    qr_data_type: paymentMethod === "SBP" ? "PAYLOAD" : undefined,
  };

  try {
    const data = await postJson("/api/tbank/payments/init", payload);
    setRailStatus(paymentMethod);
    renderPaymentPreview(data);
    writeLog(`Funding session created via ${paymentMethod}: ${data.status || "NEW"}`, "success");
  } catch (error) {
    writeLog(`Funding init failed: ${error.message}`, "error");
  }
});

els.internetForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const payload = {
    user_id: document.getElementById("internet-user-id").value.trim(),
    account_id: document.getElementById("internet-account-id").value.trim(),
    amount_minor: Number(document.getElementById("internet-amount-minor").value),
    currency: document.getElementById("internet-currency").value,
    ttl_seconds: Number(document.getElementById("internet-ttl").value),
  };

  try {
    const data = await postJson("/api/internet/credentials", payload);
    renderProviderPreview(
      "Temporary card issued",
      `TTL: ${data.expires_in_seconds || "unknown"} seconds`,
      data.provider_response || data,
    );
    writeLog("Temporary internet card credentials issued.", "success");
  } catch (error) {
    writeLog(`Temporary card issue failed: ${error.message}`, "error");
  }
});

els.permanentForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const payload = {
    user_id: document.getElementById("permanent-user-id").value.trim(),
    account_id: document.getElementById("permanent-account-id").value.trim(),
    cardholder_name: document.getElementById("permanent-name").value.trim(),
    card_type: document.getElementById("permanent-card-type").value,
    delivery_mode: document.getElementById("permanent-delivery-mode").value,
  };

  try {
    const data = await postJson("/api/cards/permanent/request", payload);
    renderProviderPreview(
      "Permanent card request created",
      `Provider HTTP status: ${data.provider_http_status || "unknown"}`,
      data.provider_response || data,
    );
    writeLog("Permanent card request sent.", "success");
  } catch (error) {
    writeLog(`Permanent card request failed: ${error.message}`, "error");
  }
});

els.authForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const payload = {
    user_id: document.getElementById("auth-user-id").value.trim(),
    account_id: document.getElementById("auth-account-id").value.trim(),
    card_id: document.getElementById("auth-card-id").value.trim(),
    merchant_id: document.getElementById("auth-merchant-id").value.trim(),
    currency: document.getElementById("auth-currency").value,
    amount_minor: Number(document.getElementById("auth-amount-minor").value),
  };

  try {
    const data = await postJson("/api/card/authorize", payload);
    updateState(data);
    renderProviderPreview(
      "Authorization result",
      `Verdict: ${data.verdict} · Frontend command: ${data.frontend_command}`,
      data,
    );
    writeLog(
      `Authorization verdict: ${data.verdict}`,
      data.verdict?.includes("Approved") ? "success" : "warn",
    );
  } catch (error) {
    writeLog(`Authorization failed: ${error.message}`, "error");
  }
});

els.kycForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const payload = {
    user_id: document.getElementById("kyc-user-id").value.trim(),
    account_id: document.getElementById("kyc-account-id").value.trim(),
    email: document.getElementById("kyc-email").value.trim(),
    origin: window.location.origin,
    return_url: document.getElementById("kyc-return-url").value.trim(),
    locale: document.getElementById("kyc-locale").value.trim(),
    applicant_type: document.getElementById("kyc-applicant-type").value,
  };

  try {
    const data = await postJson("/api/codego/kyc/session", payload);
    updateState(data);
    writeLog(
      `Codego session accepted=${data.accepted} provider_status=${data.provider_http_status}`,
      data.accepted ? "success" : "warn",
    );
    if (data.iframe_url) {
      renderCodegoIframe(data);
      writeLog("Hosted Codego iframe opened in cabinet.", "success");
    } else {
      renderProviderPreview("KYC session response", "Codego returned a session payload.", data);
    }
  } catch (error) {
    writeLog(`Codego KYC session failed: ${error.message}`, "error");
  }
});

els.sharedKycForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const rawPassportPayload = document.getElementById("shared-passport-json").value.trim();
  let passportPayloadJson = rawPassportPayload;

  try {
    passportPayloadJson = JSON.stringify(JSON.parse(rawPassportPayload));
  } catch (error) {
    writeLog(`Passport payload JSON is invalid: ${error.message}`, "error");
    return;
  }

  const payload = {
    user_id: document.getElementById("shared-user-id").value.trim(),
    account_id: document.getElementById("shared-account-id").value.trim(),
    email: document.getElementById("shared-email").value.trim(),
    origin: window.location.origin,
    return_url: `${window.location.origin}/kyc/completed`,
    passport_payload_json: passportPayloadJson,
  };

  try {
    const data = await postJson("/api/shared-kyc/process", payload);
    updateState(data);
    writeLog(
      `Shared KYC transit accepted=${data.accepted} provider_status=${data.provider_http_status}`,
      data.accepted ? "success" : "warn",
    );
    if (data.iframe_url) {
      renderCodegoIframe(data);
      writeLog("Transit KYC returned hosted iframe.", "success");
    } else {
      renderProviderPreview("Transit KYC response", "Provider returned a raw response.", data);
    }
  } catch (error) {
    writeLog(`Shared KYC transit failed: ${error.message}`, "error");
  }
});

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/service-worker.js").catch(() => {
    writeLog("Service worker registration failed.", "warn");
  });
}

window.addEventListener("message", (event) => {
  if (event.origin !== "https://kyc-sandbox.codegotech.com") {
    return;
  }
  if (event.data?.type === "kyc:done") {
    writeLog("Codego iframe reported kyc:done. Waiting for webhook sync.", "info");
  }
});

writeLog("Client cabinet ready. Funding, cards, and KYC flows are organized by scenario.", "info");
