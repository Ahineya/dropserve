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
      releases/
        20260430-120001/
        20260430-121530/
      current -> releases/20260430-121530

  nginx/
    example.com.conf
```

Single nginx hook:

```
# /etc/nginx/conf.d/dropserve.conf
include <dropserve home dir>/nginx/*.conf;
```

Everything else is isolated.

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
4. done
```

No nginx rewrite. No cert work.

---

## **delete**

```
1. on the client side, ask to type "DELETE" in an interactive shell
2. remove nginx config
3. delete site dir
4. reload nginx
```

---

## **rollback

```
switch current → previous release
```

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

