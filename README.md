This is a naive, barely tested redis module to support couchdb-memcached binary protocole

### Source 
-  https://github.com/couchbase/memcached/blob/master/docs/BinaryProtocol.md
- https://github.com/RedisLabsModules/redismodule-rs


## Build

```bash
cargo build -release
```

## Test
install some tools for testing

```bash
brew tap redis-stack/redis-stack
brew install libmemcached redis-stack-server redis
```

launch redis with the custom module

```bash
chmod a+x ./target/release/libred_couch.*
redis-server --port 6780 --loadmodule ./target/release/libred_couch.dylib
```

Expect an output like 

```bash
redis-server --port 6780 --loadmodule ./target/release/libred_couch.dylib --loadmodule /opt/homebrew/Caskroom/redis-stack-server/7.4.0-v5/lib/rejson.so
68016:C 04 Jul 2025 14:31:24.755 * oO0OoO0OoO0Oo Redis is starting oO0OoO0OoO0Oo
68016:C 04 Jul 2025 14:31:24.755 * Redis version=8.0.2, bits=64, commit=00000000, modified=1, pid=68016, just started
68016:C 04 Jul 2025 14:31:24.755 * Configuration loaded
68016:M 04 Jul 2025 14:31:24.755 * Increased maximum number of open files to 10032 (it was originally set to 2560).
68016:M 04 Jul 2025 14:31:24.755 * monotonic clock: POSIX clock_gettime
                _._                                                  
           _.-``__ ''-._                                             
      _.-``    `.  `_.  ''-._           Redis Open Source            
  .-`` .-```.  ```\/    _.,_ ''-._      8.0.2 (00000000/1) 64 bit
 (    '      ,       .-`  | `,    )     Running in standalone mode
 |`-._`-...-` __...-.``-._|'` _.-'|     Port: 6780
 |    `-._   `._    /     _.-'    |     PID: 68016
  `-._    `-._  `-./  _.-'    _.-'                                   
 |`-._`-._    `-.__.-'    _.-'_.-'|                                  
 |    `-._`-._        _.-'_.-'    |           https://redis.io       
  `-._    `-._`-.__.-'_.-'    _.-'                                   
 |`-._`-._    `-.__.-'    _.-'_.-'|                                  
 |    `-._`-._        _.-'_.-'    |                                  
  `-._    `-._`-.__.-'_.-'    _.-'                                   
      `-._    `-.__.-'    _.-'                                       
          `-._        _.-'                                           
              `-.__.-'                                               

68016:M 04 Jul 2025 14:31:24.755 # WARNING: The TCP backlog setting of 511 cannot be enforced because kern.ipc.somaxconn is set to the lower value of 128.
68016:M 04 Jul 2025 14:31:24.756 * <cbbridge> cbbridge: listener started on 11210
68016:M 04 Jul 2025 14:31:24.756 * Module 'cbbridge' loaded from ./target/release/libred_couch.dylib
[cbbridge] listening on 11210
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Created new data type 'ReJSON-RL'
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> version: 20808 git sha: unknown branch: unknown
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Exported RedisJSON_V1 API
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Exported RedisJSON_V2 API
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Exported RedisJSON_V3 API
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Exported RedisJSON_V4 API
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Exported RedisJSON_V5 API
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Enabled diskless replication
68016:M 04 Jul 2025 14:31:24.757 * <ReJSON> Initialized shared string cache, thread safe: false.
68016:M 04 Jul 2025 14:31:24.757 * Module 'ReJSON' loaded from /opt/homebrew/Caskroom/redis-stack-server/7.4.0-v5/lib/rejson.so
68016:M 04 Jul 2025 14:31:24.757 * Server initialized
68016:M 04 Jul 2025 14:31:24.757 * Loading RDB produced by version 8.0.2
68016:M 04 Jul 2025 14:31:24.757 * RDB age 1 seconds
68016:M 04 Jul 2025 14:31:24.757 * RDB memory usage when created 1.30 Mb
68016:M 04 Jul 2025 14:31:24.758 * Done loading RDB, keys loaded: 2, keys expired: 0.
68016:M 04 Jul 2025 14:31:24.758 * DB loaded from disk: 0.000 seconds
68016:M 04 Jul 2025 14:31:24.758 * Ready to accept connections tcp
```

install the pylibmc package
```bash
LIBMEMCACHED=/opt/homebrew/Cellar/libmemcached/1.0.18_2/ pip install pylibmc
pip install redis
```

then try 

```python
import pylibmc, json, redis, pprint, time

# --- 1.  binary Memcached client (port 11210) --------------------
mc = pylibmc.Client(["127.0.0.1:11210"],
                    binary=True,
                    behaviors={"tcp_nodelay": True})  # <-- binary=True !

# --- 2.  normal Redis client (port 6379) -------------------------
rd = redis.Redis(host="127.0.0.1", port=6780)

# ---------------- simple JSON ------------------------------------
doc = {"foo": "bar", "num": 1}
mc.set("doc1", json.dumps(doc))
print("doc1 via Memcached:", mc.get("doc1"))          # <-- bytes
print("doc1 via Redis    :", rd.execute_command("JSON.GET", "doc1"))

# ---------------- nested JSON ------------------------------------
nested = {"profile": {"name": "Alice", "age": 30}, "tags": ["red", "blue"]}
mc.set("user:42", json.dumps(nested))
print("user:42 via Redis :", rd.execute_command("JSON.GET", "user:42"))

# ---------------- counter (INCR) ---------------------------------
mc.incr("visits", 1, initial_value=0, time=0)
print("visits after INCR:", rd.execute_command("JSON.GET", "visits"))
```
