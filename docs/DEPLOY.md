# WarpTalk: Triển khai production (Docker Compose trên 1 VM)

Phạm vi theo ADR 0001: **copilot** (transcript, gợi ý, follow-up, biên bản). Stack gồm Postgres, Redis, Qdrant, migrator (chạy một lần), api, worker, web và (tuỳ chọn) Caddy TLS. **Không có LiveKit, không có pipeline dịch.** Biến môi trường: xem `docs/RUNBOOK.md` mục 3 (bảng đầy đủ); file này chỉ nêu phần cần cho production.

Các file liên quan:

| File | Vai trò |
|---|---|
| `deploy/docker/Dockerfile.rust` | Image Rust, mỗi binary một target: `api`, `worker`, `migrator`, `pipeline` (tuỳ chọn) |
| `deploy/docker/Dockerfile.web` | Image web (Next.js static export + Caddy, proxy `/api`) |
| `deploy/docker/docker-compose.prod.yml` | Stack production |
| `deploy/docker/Caddyfile.prod` | Reverse proxy TLS (profile `proxy`) |
| `deploy/docker/prod.env.example` | Mẫu file env (sao chép thành `.env.prod`, **không commit**) |
| `.github/workflows/{ci,docker,macos}.yml` | CI, build/push image, build app macOS |

## 1. Kiến trúc image

- **Rust**: multi-stage, `cargo-chef` cache lớp dependency, build `--release --locked` (LTO thin theo `Cargo.toml`), strip symbol. Runtime `debian:bookworm-slim` + `ca-certificates`, user không phải root (uid 10001), TLS dùng rustls (không cần libssl). Dung lượng ước tính **85-110 MB** mỗi image (đo lại bằng `docker images` sau lần build đầu). `api` có `HEALTHCHECK` gọi `GET /health` (bằng bash `/dev/tcp`, image không có curl). `worker` và `migrator` không mở cổng.
- **Web**: `apps/web/next.config.ts` chỉ cho `output: 'export'` (bản build luôn là static export `out/`), nên **`next start` không dùng được** và không có `standalone`. Image phục vụ `out/` bằng Caddy (SPA fallback, gzip/zstd, cache `_next/static`) và proxy `/api/*` tới `API_PROXY_TARGET` (mặc định `http://api:8080`, không buffer để SSE chạy được). Chạy bằng user không phải root, lắng nghe cổng 8080, `HEALTHCHECK` gọi `/healthz`. Ước tính 60-80 MB.
- Pipeline (dịch họp) **mặc định không build**: `docker build -f deploy/docker/Dockerfile.rust --build-arg WITH_PIPELINE=1 --target pipeline -t warptalk-pipeline .` và tự thêm service vào compose nếu cần (cần LiveKit, xem RUNBOOK mục 6).

## 2. Build và push image

### 2.1 Bằng GitHub Actions (khuyến nghị)

`.github/workflows/docker.yml` build 4 image (`warptalk-api`, `warptalk-worker`, `warptalk-migrator`, `warptalk-web`) và push lên `ghcr.io/<owner>/...` (owner viết thường):

- push vào `main`: tag `main`, `sha-<7 ký tự>`, `latest`;
- tag `v1.2.3`: tag `1.2.3`, `1.2`, `sha-...`;
- pull request chỉ build để kiểm Dockerfile, không push.

Phát hành: `git tag v1.2.3 && git push origin v1.2.3`. Package GHCR mặc định **private**: tạo Personal Access Token (classic) quyền `read:packages` cho VM, hoặc đặt package ở chế độ public.

### 2.2 Build tay (từ thư mục gốc repo)

```bash
R=ghcr.io/<owner>; T=v1.2.3
for t in api worker migrator; do
  docker build -f deploy/docker/Dockerfile.rust --target $t -t $R/warptalk-$t:$T .
done
docker build -f deploy/docker/Dockerfile.web -t $R/warptalk-web:$T .
echo "$GHCR_TOKEN" | docker login ghcr.io -u <user> --password-stdin
docker push $R/warptalk-api:$T   # lặp lại cho worker, migrator, web
```

Lần build Rust đầu tiên mất 10-25 phút; lần sau chỉ build lại phần code workspace nhờ cache dependency.

## 3. Triển khai lần đầu trên VM

Yêu cầu VM: Linux x86_64, 2 vCPU / 4 GB RAM trở lên, Docker Engine 25+ với Compose v2, mở cổng 80 và 443, DNS bản ghi `A`/`AAAA` của `DOMAIN` trỏ về VM (bắt buộc để Let's Encrypt cấp chứng chỉ).

```bash
git clone <repo> /opt/warptalk && cd /opt/warptalk     # chỉ cần deploy/docker/
cp deploy/docker/prod.env.example deploy/docker/.env.prod
chmod 600 deploy/docker/.env.prod
```

Điền `.env.prod`. Sinh bí mật:

```bash
openssl rand -hex 24                        # POSTGRES_PASSWORD, REDIS_PASSWORD, QDRANT_API_KEY (chỉ hex: được nhúng vào URL)
openssl rand -base64 48 | tr -d '\n=+/'     # JWT_SECRET (>= 32 byte, nếu ngắn hơn API không khởi động)
openssl rand -base64 32                     # AI_KEYS_KEK (đúng 32 byte base64)
```

Biến bắt buộc: `WARPTALK_REGISTRY` (ví dụ `ghcr.io/acme`), `WARPTALK_TAG` (ví dụ `v1.2.3`, nên ghim, tránh `latest`), `DOMAIN`, `POSTGRES_PASSWORD`, `REDIS_PASSWORD`, `JWT_SECRET`. Khuyến nghị: `QDRANT_API_KEY`, `AI_KEYS_KEK` (thiếu thì tính năng BYO key trả `503 BYO_DISABLED`), và ít nhất một trong `OPENAI_API_KEY` / `GEMINI_API_KEY` (thiếu cả hai thì embedding/Knowledge tắt). Lưu ý: **mất `AI_KEYS_KEK` là mất toàn bộ key BYO đã lưu**; sao lưu nó ở nơi an toàn, tách khỏi backup database.

```bash
C="docker compose -f deploy/docker/docker-compose.prod.yml --env-file deploy/docker/.env.prod"
echo "$GHCR_TOKEN" | docker login ghcr.io -u <user> --password-stdin   # nếu package private
$C config -q                    # kiểm tra cấu hình, báo biến còn thiếu
$C pull
$C --profile proxy up -d        # migrator chạy trước, api/worker chờ migrator hoàn tất
$C ps
curl -fsS -o /dev/null -w '%{http_code}\n' https://$DOMAIN/        # web: 200
$C exec api bash -c 'exec 3<>/dev/tcp/127.0.0.1/8080; printf "GET /health HTTP/1.0\r\n\r\n" >&3; head -n1 <&3'   # api: HTTP/1.0 200
```

Không có Caddy (đã có reverse proxy riêng, ví dụ nginx/ALB): bỏ `--profile proxy`. Container `web` được publish ở `127.0.0.1:${WEB_PORT:-3000}`; trỏ proxy của bạn về đó (chỉ cần một upstream, vì `web` đã proxy `/api` sang `api`). Với proxy riêng nhớ tắt buffering cho `/api` (SSE `/meetings/{id}/suggestions/stream`).

Postgres, Redis, Qdrant **không publish cổng ra ngoài**; chỉ truy cập qua mạng compose (`docker compose exec ...`).

> **Lưu ý email**: chưa có mailer thật trong bản hiện tại (RUNBOOK mục 3.2): API không gửi được email xác minh/lời mời. `DEV_MAIL_CAPTURE=1` chỉ lưu token thô vào Redis để dev đọc và **không được bật ở production**. Cần có nhà cung cấp email trước khi mở đăng ký công khai; trong lúc chờ, xác minh tài khoản thủ công bằng SQL do quản trị viên thực hiện.

## 4. Migration

`migrator` là service chạy một lần (`restart: "no"`) với cùng `DATABASE_URL`; `api` và `worker` có `depends_on: migrator: service_completed_successfully` nên luôn chạy sau khi migration xong. Migration là **forward-only**, có checksum: tuyệt đối không sửa file migration đã áp dụng.

```bash
$C logs migrator                          # kết quả lần chạy gần nhất
$C run --rm migrator                      # chạy lại tay (idempotent)
```

Nếu migrator thoát lỗi, `api`/`worker` không khởi động (đó là hành vi mong muốn); đọc log rồi xử lý, đừng bỏ qua dependency.

## 5. Backup và khôi phục

**Postgres** (nguồn dữ liệu chính):

```bash
mkdir -p /var/backups/warptalk
$C exec -T postgres sh -c 'pg_dump -U "$POSTGRES_USER" -Fc "$POSTGRES_DB"' \
  > /var/backups/warptalk/pg-$(date +%F-%H%M).dump
# Cron hằng đêm + giữ 14 bản + đẩy ra object storage ngoài VM.
```

Khôi phục (vào DB trống; dừng api/worker trước):

```bash
$C stop api worker web
$C exec -T postgres sh -c 'dropdb -U "$POSTGRES_USER" --if-exists "$POSTGRES_DB" && createdb -U "$POSTGRES_USER" "$POSTGRES_DB"'
$C exec -T postgres sh -c 'pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" --no-owner' < /var/backups/warptalk/pg-XXXX.dump
$C up -d
```

Cần sao lưu thêm:

| Dữ liệu | Cách |
|---|---|
| Volume `objects` (tài liệu upload, khi `STORAGE_PROVIDER=local`) | `docker run --rm -v warptalk-prod_objects:/d -v $PWD:/b alpine tar czf /b/objects.tgz -C /d .` (hoặc dùng S3: `STORAGE_PROVIDER=s3`) |
| Volume `qdrantdata` (vector Knowledge) | Có thể dựng lại từ transcript bằng cách re-index; nếu muốn giữ: snapshot Qdrant (`POST /snapshots`) hoặc tar volume khi đã dừng qdrant |
| Volume `redisdata` | Chỉ là stream/cache/trạng thái tạm (AOF bật); không bắt buộc |
| `.env.prod`, đặc biệt `AI_KEYS_KEK` và `JWT_SECRET` | Lưu trong trình quản lý bí mật; đổi `JWT_SECRET` sẽ đăng xuất mọi người dùng |
| Volume `caddy_data` | Chứa chứng chỉ TLS; mất thì Caddy tự xin lại (chú ý giới hạn tần suất của Let's Encrypt) |

Thử khôi phục định kỳ trên VM khác: backup chưa từng khôi phục thì chưa phải backup.

## 6. TLS

Profile `proxy` chạy Caddy: tự xin và gia hạn chứng chỉ Let's Encrypt cho `DOMAIN` (cổng 80 dùng cho ACME HTTP-01 và chuyển hướng, 443 cho HTTPS/HTTP3), thêm HSTS. `/api/*` đi thẳng tới `api:8080` (không nén, không buffer, an toàn cho SSE), còn lại tới `web:8080`. Kiểm thử không cần DNS thật: `DOMAIN=localhost` (Caddy dùng CA nội bộ, trình duyệt sẽ cảnh báo). Chứng chỉ nằm trong volume `caddy_data`.

## 7. Nâng cấp và rollback

Nâng cấp (downtime vài giây, ngắn vì api/worker khởi động lại):

```bash
$C exec -T postgres sh -c 'pg_dump -U "$POSTGRES_USER" -Fc "$POSTGRES_DB"' > /var/backups/warptalk/pre-upgrade.dump   # luôn backup trước
sed -i 's/^WARPTALK_TAG=.*/WARPTALK_TAG=v1.3.0/' deploy/docker/.env.prod
$C pull
$C --profile proxy up -d        # migrator chạy lại, rồi api/worker/web được tạo lại với image mới
$C ps && $C logs --tail=100 api worker
```

Rollback:

1. Đặt lại `WARPTALK_TAG` về bản trước, `$C pull && $C --profile proxy up -d`.
2. Migration là forward-only: image cũ phải chạy được với schema mới. Quy ước: migration chỉ thêm (additive), không xoá/đổi tên cột trong cùng bản phát hành với code còn dùng nó. Nếu bản mới có migration phá vỡ tương thích thì rollback = khôi phục `pre-upgrade.dump` (mục 5) cùng với image cũ.
3. Ghi lại sự cố, sửa tiến (forward fix) bằng migration mới thay vì sửa migration đã chạy.

## 8. Vận hành

- Log: `$C logs -f api worker` (JSON, `LOG_FORMAT=json`; docker xoay vòng 5 x 20 MB). Chỉnh mức log bằng `RUST_LOG`.
- Metrics Prometheus: đặt `METRICS_BIND` (ví dụ `0.0.0.0:9090` trong container api) và scrape trong mạng nội bộ; **không** publish ra Internet. Compose không bật mặc định.
- Tài nguyên: `docker stats`. Postgres là thành phần cần RAM/đĩa nhất; theo dõi dung lượng volume `pgdata`.
- Dọn image cũ: `docker image prune -f` sau mỗi lần nâng cấp thành công.
- Bí mật: `.env.prod` quyền `600`, thuộc root, ngoài git. CI quét bí mật bằng gitleaks (NFR-SC-05).

## 9. Phát hành app macOS

Workflow `.github/workflows/macos.yml` (chạy tay qua *Actions > macOS app > Run workflow* hoặc khi push tag `v*`) trên runner `macos-15`, Xcode 16.4:

1. Cài target Rust `aarch64-apple-darwin`, `x86_64-apple-darwin`; build web export (`apps/web/out`) để nhúng vào app.
2. `make core` (copilot-core thành `CopilotCore.xcframework` + binding UniFFI), `make gen` (XcodeGen), `make test` (XCTest).
3. Build Release có `REQUIRE_WEB_BUNDLE=YES` (nhúng web, `warptalk://app/`), đóng gói `WarpTalk-macos.zip` và upload artifact (giữ 30 ngày).
4. **Mặc định không ký.** Nếu có secrets bên dưới, workflow tự ký Developer ID (hardened runtime) và notarize + staple.

| Secret (repo/environment) | Nội dung |
|---|---|
| `APPLE_CERTIFICATE_P12_BASE64` | Chứng chỉ "Developer ID Application" xuất `.p12`, mã hoá base64 (`base64 -i cert.p12 \| pbcopy`) |
| `APPLE_CERTIFICATE_PASSWORD` | Mật khẩu của `.p12` |
| `APPLE_SIGNING_IDENTITY` | Ví dụ `Developer ID Application: Tên Công Ty (TEAMID)` |
| `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD` | Cho `notarytool` (mật khẩu riêng cho ứng dụng tạo ở appleid.apple.com) |

Việc cần làm trước bản phát hành thật (nêu trong `apps/macos/README.md` mục 2): đổi bundle id placeholder `com.example.warptalk` trong `project.yml`, bật lại App Sandbox / siết `NSAllowsArbitraryLoads` trong entitlements và Info.plist, và đặt "URL API" mặc định là `https://<DOMAIN>` (bản nhúng proxy `warptalk://app/api/**` tới URL này; SSE chưa được hỗ trợ ở đường proxy nhúng, xem README macOS mục 3). Phát hành: tải artifact, kiểm `spctl -a -vv WarpTalk.app` và `xcrun stapler validate`, rồi đính kèm vào GitHub Release hoặc kênh phân phối riêng.

## 10. CI (tóm tắt)

`ci.yml` (push, PR, hằng đêm): `rust` (fmt, clippy `-D warnings`, test với Postgres 16 + Redis 7), `web` (typecheck, lint, `next build`), `e2e` (Playwright chromium với backend thật), `secrets-scan` (gitleaks), `audit` (npm audit + cargo audit, không chặn). Hằng đêm/dispatch có thêm `e2e-extended` (các project `export`, `meeting-media`, `macos-webview`). Mọi action ghim theo tag phiên bản đầy đủ; việc tiếp theo: ghim theo commit SHA (Dependabot hoặc `pinact`). Nên bật *Branch protection* yêu cầu các job `rust`, `web`, `e2e`, `secrets-scan` xanh trước khi merge vào `main`. Repo thuộc tổ chức (organization) cần secret `GITLEAKS_LICENSE` cho gitleaks-action.
