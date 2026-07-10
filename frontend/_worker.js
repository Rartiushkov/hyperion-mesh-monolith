function jsonError(message, status = 502) {
  return new Response(JSON.stringify({ error: message }), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
    },
  });
}

function normalizeOrigin(origin, scheme) {
  if (!origin) {
    return "";
  }
  if (origin.startsWith("http://") || origin.startsWith("https://")) {
    return origin.replace(/\/$/, "");
  }
  return `${scheme}://${origin.replace(/\/$/, "")}`;
}

function resolveRoute(url, env) {
  const scheme = env.API_SCHEME || "http";
  const publicOrigin = normalizeOrigin(
    env.PUBLIC_API_ORIGIN || "housing-contribution-gonna-tuesday.trycloudflare.com",
    scheme,
  );

  if (url.pathname === "/api/aaio/webhook") {
    return {
      origin: publicOrigin,
      path: "/aaio/webhook",
    };
  }

  if (url.pathname.startsWith("/api/")) {
    return {
      origin: publicOrigin,
      path: url.pathname.replace(/^\/api/, ""),
    };
  }

  return null;
}

function upstreamOrigin(env) {
  const scheme = env.API_SCHEME || "http";
  const origin =
    env.PUBLIC_API_ORIGIN ||
    env.API_ORIGIN ||
    "housing-contribution-gonna-tuesday.trycloudflare.com";
  if (!origin) {
    return "";
  }
  return normalizeOrigin(origin, scheme);
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.pathname.startsWith("/api/")) {
      const route = resolveRoute(url, env);
      const origin = route?.origin || upstreamOrigin(env);
      if (!origin || !route) {
        return jsonError("API_ORIGIN is not configured in Cloudflare.");
      }
      const target = new URL(route.path, origin);
      target.search = url.search;
      const proxyRequest = new Request(target.toString(), request);
      proxyRequest.headers.set("host", new URL(origin).host);
      return fetch(proxyRequest, {
        cf: {
          cacheTtl: 0,
          cacheEverything: false,
        },
      });
    }

    return env.ASSETS.fetch(request);
  },
};
