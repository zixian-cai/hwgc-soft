#!/usr/bin/env bash

set -ex

function do_rsync {
    local src=$2
    local dst=$1:/home/$USER
    rsync -azR --no-i-r -h --info=progress2 $src $dst
}

remote=$1
remote_path=$remote:/home/$USER
root=/home/$USER/./hwgc-soft

do_rsync $remote $root/sampled
do_rsync $remote $root/scripts
do_rsync $remote $root/builds