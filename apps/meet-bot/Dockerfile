# Meet bot: headed Chromium on a virtual X display (Xvfb), real X input via xdotool.
# Nothing is shown on the host; Meet sees an ordinary desktop browser.
FROM node:22-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      chromium xvfb xdotool pulseaudio imagemagick fonts-noto-core ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
RUN npm init -y >/dev/null && npm install --omit=dev playwright-core@1.63.0 && npm cache clean --force
COPY join.cjs bot.cjs entrypoint.sh ./
RUN chmod +x entrypoint.sh && mkdir -p /out && chown node /out
USER node
ENV HIDE=xvfb CHROME_PATH=/usr/bin/chromium DISPLAY=:99 SHOT_DIR=/out
ENTRYPOINT ["/app/entrypoint.sh"]
