# must run in new rootfs root path /
# https://www.kernel.org/doc/Documentation/filesystems/sharedsubtree.txt
mount --make-rslave / # make mount not visible in parent
mkdir -p tmp/old_root
mount --rbind /dev dev/
mount -t proc proc proc # virtual fs 
mount -t sysfs sys sys # virtual fs
pivot_root . tmp/old_root
cd /
umount -l /tmp/old_root
bash
