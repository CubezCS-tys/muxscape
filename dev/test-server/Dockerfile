# Disposable SSH + tmux server used for Muxscape integration tests.
# Never point tests at a real tmux server.
ARG BASE=ubuntu:24.04
FROM ${BASE}
ARG WITH_TMUX=1
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      openssh-server vim-tiny htop less procps locales ca-certificates \
    && if [ "$WITH_TMUX" = "1" ]; then apt-get install -y --no-install-recommends tmux; fi \
    && rm -rf /var/lib/apt/lists/* \
    && sed -i 's/# en_US.UTF-8/en_US.UTF-8/' /etc/locale.gen && locale-gen \
    && mkdir -p /run/sshd \
    && useradd -m -s /bin/bash dev && echo 'dev:devpass' | chpasswd
COPY authorized_keys /home/dev/.ssh/authorized_keys
RUN chown -R dev:dev /home/dev/.ssh && chmod 700 /home/dev/.ssh && chmod 600 /home/dev/.ssh/authorized_keys \
 && printf 'PasswordAuthentication yes\nAcceptEnv LANG LC_*\nAllowTcpForwarding yes\n' > /etc/ssh/sshd_config.d/muxscape.conf \
 && echo 'export LANG=en_US.UTF-8' > /etc/profile.d/lang.sh
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
