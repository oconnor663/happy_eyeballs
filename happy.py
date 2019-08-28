#! /usr/bin/env python3

# Code from https://youtu.be/oLkfnc_UMcE

import trio

async def open_tcp_socket(hostname, port, *, max_wait_time=0.250):
    targets = await trio.socket.getaddrinfo(
        hostname, port, type=trio.socket.SOCK_STREAM)
    failed_attempts = [trio.Event() for _ in targets]
    winning_socket = None

    async def attempt(target_idx, nursery):
        # wait for previous one to finish, or timeout to expire
        if target_idx > 0:
            with trio.move_on_after(max_wait_time):
                await failed_attempts[target_idx - 1].wait()

        # start next attempt
        if target_idx + 1 < len(targets):
            nursery.start_soon(attempt, target_idx + 1, nursery)

        # try to connect to our target
        try:
            *socket_config, _, target = targets[target_idx]
            socket = trio.socket.socket(*socket_config)
            await socket.connect(target)
        # if fails, tell next attempt to go ahead
        except OSError:
            failed_attempts[target_idx].set()
        else:
            # if succeeds, save winning socket
            nonlocal winning_socket
            winning_socket = socket
            # and cancel other attempts
            nursery.cancel_scope.cancel()

    async with trio.open_nursery() as nursery:
        nursery.start_soon(attempt, 0, nursery)

    if winning_socket is None:
        raise OSError("ruh-oh")
    else:
        return winning_socket

# Let's try it out:
async def main():
    print(await open_tcp_socket("google.com", "https"))

trio.run(main)
