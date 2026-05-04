# dropserve

**Artifact + domain → HTTPS site.** No build pipelines or hosting UI: you zip static files, ship them over **SSH/SCP**, and **nginx + certbot** do the rest.

---

## Install the CLI

From this repo:

```bash
cargo install --path .
```

That installs the **`dropserve`** binary (the Rust package name is `ds`).

Or run without installing:

```bash
cargo run --bin dropserve -- <ARGS>
```

### Cross-compile for Linux (x86_64)

From macOS (or another host) with [Zig](https://ziglang.org) and [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild):

```bash
./scripts/build-linux-x86_64.sh
```

Output: `target/x86_64-unknown-linux-gnu/release/dropserve`

### Prebuilt binaries (GitHub Releases)

CI publishes **`dropserve-<version>-linux-x86_64`**, **`linux-aarch64`**, and **`macos-aarch64`** `.tar.gz` files (plus `.sha256`) when you push a Git tag **`vX.Y.Z`** whose **`X.Y.Z`** matches **`version`** in **`Cargo.toml`**:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Check the build: **`dropserve --version`** (or **`-V`**) prints that Cargo package version.

Copy that binary onto your server and ensure it is on **`PATH`** for the account you deploy with (often `root`), because remote commands run as:

```text
ssh user@host dropserve remote …
```

---

## One-time server setup

Run **as root** (or any user that can edit nginx’s main config and reload nginx):

```bash
dropserve serve /var/www/dropserve
```

Use a directory **outside `/root`** unless you know nginx’s worker user (usually **`www-data`**) can traverse every directory down to `sites/` (see [Permissions](#permissions)).

What **`dropserve serve`** does:

- Ensures **certbot** and **nginx** exist on `PATH`
- Creates the tree under your chosen **main directory** (below)
- Writes **`/etc/dropserve/home`** with that directory’s canonical path so **`dropserve remote`** over SSH (non-login shells) uses the **same** tree nginx includes—no need to set `DROPSERVE_HOME` unless you want an override
- Adds an **`include …/dropserve.conf;`** line **inside** the `http { … }` block of **`--nginx-conf`** (default `/etc/nginx/nginx.conf`) if missing
- Runs **`nginx -t`** (plain or via **`sudo`**)

Optional flags:

```bash
dropserve serve /var/www/dropserve --nginx-conf /etc/nginx/nginx.conf
```

---

## Client commands (from your laptop or CI)

Global option:

| Option | Meaning |
|--------|--------|
| `--ssh-key PATH` | SSH private key (or set **`DROPSERVE_SSH_KEY`**) |

Replace `deploy@host` with your **`SSH user@hostname-or-IP`**.

| Command | What it does |
|--------|----------------|
| **`dropserve create deploy@host example.com ./dist`** | Zip `./dist` (or upload a `.zip` as-is), upload over SCP, extract release, write nginx snippet, reload nginx, wait until DNS **A** matches server public IP, run **certbot --nginx** |
| **`dropserve create deploy@host example.com ./dist --pocketbase --pb-port 8090`** | Create a PocketBase-backed site: nginx proxies to a generated systemd service on `127.0.0.1:8090`; releases still use the same `current` symlink model |
| **`dropserve update deploy@host example.com ./dist`** | New timestamped release + atomic flip of the **`current`** symlink |
| **`dropserve delete deploy@host example.com`** | Prompts you to type **`DELETE`**, then removes vhost snippet and site directory |
| **`dropserve list deploy@host`** | Lists sites, current release id, and deployment type |
| **`dropserve rollback deploy@host example.com`** | Points **`current`** at the previous release |

**Artifact:** `create` / `update` accept either a **directory** (zipped on the fly) or an existing **`.zip`** file.

### PocketBase sites

PocketBase support intentionally stays inside the same Dropserve loop:

```bash
dropserve create deploy@host app.example.com ./dist --pocketbase --pb-port 8090
dropserve update deploy@host app.example.com ./dist
dropserve rollback deploy@host app.example.com
dropserve delete deploy@host app.example.com
```

Server requirements:

- `pocketbase` must be installed on the server and available on `PATH`
- the deploy user must be able to run `systemctl` directly or through `sudo`
- choose a unique `--pb-port` per PocketBase-backed site

Release conventions:

- if the uploaded artifact already contains `pb_public/`, Dropserve leaves it in place
- otherwise Dropserve moves the artifact contents into `pb_public/`
- `pb_migrations/` is preserved when present and created empty otherwise
- persistent data lives at `sites/<domain>/pb_data`

Dropserve writes a small `sites/<domain>/dropserve-site.conf` file so later `update`, `rollback`, `delete`, and `list` know the site is PocketBase-backed. Updates and rollbacks restart the generated PocketBase service after flipping `current`.

`dropserve list deploy@host` prints deployment type inline:

```text
example.com	current=20260504-120000	type=static
app.example.com	current=20260504-121500	type=backend	backend=pocketbase	port=8090
```

---

## Where files live on the server

If main directory is `/var/www/dropserve`:

```text
/var/www/dropserve/
  dropserve.conf          # include nginx/*.conf
  nginx/
    example.com.conf      # generated server block (certbot may edit this)
  systemd/
    dropserve-example-com-pocketbase.service  # generated for PocketBase sites
  sites/
    example.com/
      dropserve-site.conf # runtime metadata
      pb_data/            # persistent PocketBase data when --pocketbase is used
      releases/
        YYYYMMDD-HHMMSS/
      current -> releases/…   # symlink; nginx root points here
/etc/dropserve/home         # written by dropserve serve; used by remote SSH commands
```

Vhosts are **not** dropped into `sites-enabled/`; they are included via **`dropserve.conf`**.

---

## Environment variables

| Variable | Where | Purpose |
|----------|--------|--------|
| **`DROPSERVE_SSH_KEY`** | Client | Default SSH private key path |
| **`DROPSERVE_HOME`** | Server | Overrides dropserve tree for **`dropserve remote`**; if unset, **`/etc/dropserve/home`** (from **`dropserve serve`**) is used, then **`~/.dropserve`** |

---

## DNS and TLS

- Point your domain’s **A** record at the server **before** or **during** the first **`create`** (the server polls with **`dig`** until it matches the machine’s public IPv4 from **`curl`**).
- **certbot** runs non-interactively with **`--register-unsafely-without-email`** for automation; adjust if your policy requires an email.

---

## Permissions

If nginx returns **403** or **500** for static files, check **`/var/log/nginx/error.log`** for **`Permission denied`**.

The worker must be able to **traverse** every directory from `/` down to **`…/sites/<domain>/current`**. Avoid putting the main directory under **`/root`** unless you deliberately relax directory execute bits or align ownership with **`www-data`**.

---

## Releases

- Each deploy is a **timestamped** folder under **`releases/`**.
- **`current`** is updated **atomically** via symlink rename.
- Older releases are pruned automatically (**last 5** kept).

---

## Troubleshooting

| Symptom | Things to check |
|--------|-------------------|
| Certbot “could not find a matching server block” | Same tree for nginx **and** remote: run **`dropserve serve`** again so **`/etc/dropserve/home`** matches; confirm **`nginx -T`** shows your `server_name` from **`{main}/nginx/<domain>.conf`**. |
| Wrong directory used over SSH | Contents of **`/etc/dropserve/home`** and optional **`DROPSERVE_HOME`**. |
| nginx syntax / include errors | **`sudo nginx -t`**; ensure dropserve’s **`include`** sits **inside** `http { }`. |

---

## Design note

See **`DOC.md`** for the original product sketch. This implementation matches that model: **SSH + symlink releases + nginx + certbot**.
