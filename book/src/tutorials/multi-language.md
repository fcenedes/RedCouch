# Multi-Language Examples

RedCouch speaks standard memcached protocol, so any memcached client library works. This chapter shows working examples in several languages, all connecting to RedCouch on port 11210.

> **Prerequisite:** Redis 8+ with RedCouch loaded and listening on `127.0.0.1:11210`. See [Installation](../getting-started/installation.md).

---

## Python (pymemcache)

The full Python walkthrough is in the [Python Client Tutorial](./python-client.md). Here's the quick version:

```python
# pip install pymemcache
from pymemcache.client.base import Client

client = Client(("127.0.0.1", 11210))
client.set("py-key", "hello from python")
print(client.get("py-key"))  # b'hello from python'

# CAS workflow
value, cas = client.gets("py-key")
client.cas("py-key", "updated", cas)

# Counters
client.set("hits", "0")
client.incr("hits", 1)

client.close()
```

---

## Node.js (memjs)

[memjs](https://github.com/memcachier/memjs) is a popular Node.js memcached client that supports binary protocol with SASL authentication — ideal for RedCouch since it exercises the binary protocol path including SASL.

```javascript
// npm install memjs

const memjs = require('memjs');

// memjs uses binary protocol with SASL auth by default.
// RedCouch's SASL is stub-only, so any username/password works.
const client = memjs.Client.create('127.0.0.1:11210', {
  username: 'any',
  password: 'any'
});

async function main() {
  // Store a value
  await client.set('node-key', 'hello from node.js', { expires: 60 });

  // Retrieve it
  const { value, flags } = await client.get('node-key');
  console.log(value.toString()); // 'hello from node.js'

  // Delete
  await client.delete('node-key');

  client.close();
}

main().catch(console.error);
```

> **Note:** memjs uses the memcached **binary protocol**, so this example exercises RedCouch's binary protocol path including SASL authentication handshake.

---

## Go (gomemcache)

[gomemcache](https://github.com/bradfitz/gomemcache) by Brad Fitzpatrick (original memcached author) is the standard Go memcached client. It uses the ASCII text protocol.

```go
// go get github.com/bradfitz/gomemcache/memcache
package main

import (
    "fmt"
    "github.com/bradfitz/gomemcache/memcache"
)

func main() {
    // Connect to RedCouch
    mc := memcache.New("127.0.0.1:11210")

    // Store a value with 60-second TTL
    mc.Set(&memcache.Item{
        Key:        "go-key",
        Value:      []byte("hello from go"),
        Expiration: 60,
    })

    // Retrieve it
    item, err := mc.Get("go-key")
    if err != nil {
        panic(err)
    }
    fmt.Println(string(item.Value)) // "hello from go"

    // CAS workflow
    item, _ = mc.Get("go-key")
    item.Value = []byte("updated from go")
    err = mc.CompareAndSwap(item)
    fmt.Printf("CAS update: %v\n", err == nil)

    // Counters
    mc.Set(&memcache.Item{Key: "go-counter", Value: []byte("0")})
    newVal, _ := mc.Increment("go-counter", 5)
    fmt.Printf("Counter: %d\n", newVal) // 5

    // Delete
    mc.Delete("go-key")
}
```

---

## PHP (ext-memcached)

PHP's `Memcached` extension uses the **binary protocol** via libmemcached. It works with RedCouch out of the box:

```php
<?php
$mc = new Memcached();
$mc->addServer('127.0.0.1', 11210);

// Enable binary protocol (recommended for RedCouch)
$mc->setOption(Memcached::OPT_BINARY_PROTOCOL, true);

// Store and retrieve
$mc->set('php-key', 'hello from php', 60);
echo $mc->get('php-key') . "\n"; // 'hello from php'

// CAS workflow
$cas = null;
$value = $mc->get('php-key', null, Memcached::GET_EXTENDED);
$mc->cas($value['cas'], 'php-key', 'updated from php');

// Counters
$mc->set('php-counter', 0);
$mc->increment('php-counter', 5);
echo $mc->get('php-counter') . "\n"; // 5
```

---

## CLI Tools (telnet / netcat)

The fastest way to test RedCouch is with `telnet` or `nc`. These connect via ASCII protocol:

```bash
# Connect
telnet 127.0.0.1 11210

# Store a value (set <key> <flags> <exptime> <bytes>)
set cli-key 0 0 9
cli-value
STORED

# Retrieve
get cli-key
VALUE cli-key 0 9
cli-value
END

# Use meta protocol
ms meta-key 5
hello
HD

mg meta-key v c
VA 5 c1
hello

# Quit
quit
```

See the [ASCII Protocol](../guide/ascii-protocol.md) and [Meta Protocol](../guide/meta-protocol.md) guides for comprehensive command references.

---

## Choosing a Client Library

| Language | Library | Protocol | Auth | Notes |
|---|---|---|---|---|
| Python | pymemcache | ASCII | No | Simple, well-maintained, pure Python |
| Node.js | memjs | Binary | SASL | Exercises binary+SASL path |
| Go | gomemcache | ASCII | No | By the original memcached author |
| PHP | ext-memcached | Binary | SASL | Via libmemcached; widely deployed |
| CLI | telnet / nc | ASCII | N/A | Quick testing and debugging |
| Any | Raw sockets | Binary | Optional | Full control; see [Binary Protocol](../guide/binary-protocol.md) |

> **Tip:** All these libraries connect to RedCouch exactly as they would to a standard memcached server — just change the port to 11210. No RedCouch-specific client code is needed.

## Next Steps

- **[Python Client Tutorial](./python-client.md)** — Deep-dive Python walkthrough with CAS, counters, pipelining
- **[Migration Guide](./migration-guide.md)** — Step-by-step path from memcached to native Redis
- **[Use Cases](./use-cases.md)** — Real-world scenarios and patterns
