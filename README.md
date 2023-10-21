<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://static.ringsnetwork.io/ringsnetwork_logo.png">
  <img alt="Rings Network" src="https://raw.githubusercontent.com/RingsNetwork/asserts/main/logo/rings_network_red.png">
</picture>

Rings Portal (The Tcp Protal implementation based on Rings)
===============

Rings Portal is a TcpProxy implementation based on the Rings network. It allows for the anonymous establishment of tunnels on the network in a centralized manner, aiming to offer decentralized services with enhanced privacy and security.


# Introduction

Imagine a service provider, Alice, who wishes to offer an IPFS API service. In a traditional environment, she'd be compelled to disclose her actual IP address, incurring numerous privacy risks. But with Rings Portal, Alice only needs to reveal her Decentralized Identifier (Did), often tied to her Ethereum address.

When Alice wants her service to be discoverable, she connects to the Rings Distributed Hash Table (DHT). Similarly, a potential service consumer, Bob, connects to the same DHT. Now, when Bob seeks a particular service, he can effortlessly search the DHT and locate services offered by providers like Alice. Importantly, Alice doesn't have to reveal any additional personal details, thanks to the anonymity provided by her Did. In essence, as long as both Alice and Bob are connected to Rings DHT, they can securely offer and consume services with utmost privacy.

Rings Portal, thus, bridges the gap between decentralized service providers and consumers, eliminating the hurdles and risks associated with traditional service discovery and connection methods.

# Implementation Details of Rings Portal

Rings Portal uses Chord DHT for service discovery and registration, employs the Rings Relay network to implement Tcp Tunnel, and harnesses WebRTC Transport to achieve service penetration. Below are the implementation details.

### Tcp Proxy Signals

Rings Portal operates based on three fundamental signals:
* TcpDial: Used to initiate a Tcp connection.
* TcpClose: Employed to terminate a Tcp connection.
* TcpPackage: Facilitates the sending of Tcp packets.

### TcpProxy Realization via Backend

At its core, Rings Portal operates as a decentralized reverse proxy. The two ends of this proxy respectively emulate the roles of a traditional tcp proxy's client and server. When a Client desires access to a particular resource via the Server, the communication ensues by connecting through the Server's Did, facilitating message transmission and reception.

### Service Discovery and Registration via Chord DHT
Rings Portal harnesses the capabilities of Rings DHT (Distributed Hash Table) for service registration and discovery. For instance, if one wishes to register a service named "ipfs_provider", it will be logged in the DHT with the following key-value pair: hash(ipfs_provider) to Did (Decentralized Identifier).

### Network Communication via Rings Relay
To establish network communication, Rings Portal leverages the routing algorithm of Rings DHT. The Chord algorithm plays a pivotal role in this process. Consequently, the expected ideal number of relays for communication is six.
