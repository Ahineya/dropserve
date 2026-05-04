# **🔥 dropserve — recap**

## **Core idea**

```
artifact (a folder contents or a .zip file)
+ domain
→ HTTPS website
```

That’s it.

No builds. No pipelines. No platform. No accounts.

---

# **🧩 Components**

## **1. CLI (client)**

Runs locally or in CI.

```
dropserve create deploy@host site.com ./dist
dropserve create deploy@host site.com ./dist --pocketbase --pb-port 8090
dropserve update deploy@host site.com ./dist
dropserve delete deploy@host site.com
dropserve list deploy@host
```

Responsibilities:

```
- zip ./dist (or just take an archive)
- send to remote host (scp/ssh)
- trigger deploy command remotely
```

`list` stays intentionally plain:

```
example.com	current=20260504-120000	type=static
app.example.com	current=20260504-121500	type=backend	backend=pocketbase	port=8090
```

No HTTP API required. Just SSH. Allow specifying the path to ssh key via env var or --ssh-key param

---

## **2. Remote binary (server mode)**

Same executable, different mode:

```
dropserve serve [main-dir]
```

Responsibilities:

```
- receive artifact
- manage releases
- write nginx configs
- handle certs
- reload nginx
```

---

# **📁 Filesystem layout**

```
<dropserve main directory, by default ~/.dropserve>
  sites/
    example.com/
      dropserve-site.conf
      pb_data/
      releases/
        20260430-120001/
        20260430-121530/
      current -> releases/20260430-121530

  nginx/
    example.com.conf
  systemd/
    dropserve-example-com-pocketbase.service
```

Single nginx hook:

```
# /etc/nginx/conf.d/dropserve.conf
include <dropserve home dir>/nginx/*.conf;
```

Everything else is isolated.

PocketBase-backed sites use the same release tree. The only added persistent state is `sites/<domain>/pb_data`, plus a tiny runtime metadata file so updates and rollbacks can restart the generated service.

---

# **🚀 Deploy flow**

## **create**

```
1. upload artifact
2. extract to releases/<timestamp>/
3. create nginx config (HTTP)
4. enable site
5. wait for DNS → IP match
6. run certbot
7. reload nginx
```

---

## **update**

```
1. upload artifact
2. extract new release
3. switch symlink (atomic)
4. restart PocketBase service if the site uses --pocketbase
5. done
```

No nginx rewrite. No cert work.

---

## **delete**

```
1. on the client side, ask to type "DELETE" in an interactive shell
2. stop/disable generated PocketBase service if present
3. remove nginx config
4. delete site dir
5. reload nginx
```

---

## **rollback

```
switch current → previous release
```

---

# **🗄️ PocketBase mode**

Optional:

```
dropserve create deploy@host app.example.com ./dist --pocketbase --pb-port 8090
```

This changes only the runtime wiring:

```
nginx → http://127.0.0.1:<pb-port> → generated systemd PocketBase service
```

The deployment model remains:

```
artifact → releases/<timestamp>/ → current symlink
```

Artifact convention:

```
pb_public/       # served by PocketBase; optional if the artifact is already the public build
pb_migrations/   # optional
```

If `pb_public/` is missing, Dropserve moves the uploaded artifact contents into `pb_public/`. Persistent PocketBase data lives outside releases at `sites/<domain>/pb_data`.

---

# **🌐 Nginx config (generated)**

```
server {
    listen 80;
    server_name example.com;

    root <dropserve main directory>/sites/example.com/current;
    index index.html;

    location / {
        try_files $uri $uri/ /index.html;
    }
}
```

After cert:

```
listen 443 ssl http2;
```

---

# **🔐 TLS flow**

```
install site (HTTP)
→ poll DNS (dig)
→ when A record matches server IP:
   certbot --nginx -d domain
→ reload nginx
```

No DNS API integration. No provider lock-in.

---

# **🔄 Release model**

```
timestamped releases
+ current symlink
+ keep last 5
```

Benefits:

- atomic deploys
- instant rollback
- no downtime
- no state machine

---

# **🧭 Philosophy**

```
files in → files served
```

Everything else is noise.

---

# **🧨 One-line summary**

```
dropserve = “ssh/scp + symlink + nginx + certbot”, packaged as a tool
```
