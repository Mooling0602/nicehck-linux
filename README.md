# NICEHCK / YUANDAO USB-C 耳机 Linux 原生控制工具

在 Linux 上配置 **NICEHCK / 原道有线 USB-C DSP 耳机**（十周年纪念版、Octave、PureAural、
StringSnow、Tears、NK1 系列等）的原生软件包。

本项目的协议实现来自对官方 Android 应用 `com.yuandao.nicehck` **2.3.8 的静态逆向**，
不依赖任何厂商文档。协议推导过程与证据见 [`docs/PROTOCOL.md`](docs/PROTOCOL.md)。

> 与 NICEHCK / 原道无任何隶属关系。刷写固件、写入 DSP 均自行承担风险。

---

## 它做什么

耳机通过一条 **USB HID 厂商接口**（`bInterfaceClass = 0x03`）接受控制指令。
手机 App 在这个链路里只是一个 HID 客户端，所以 Linux 完全可以用同样的方式驱动它。

| | |
|---|---|
| 传输 | `/dev/hidraw*`，64 字节报告 |
| 控制报告 ID | `0x4B` |
| 固件报告 ID | `0x54` |
| EQ 模型 | RBJ peaking biquad，系数以 signed Q30 下发 |
| 采样率 | 96000 Hz |

十周年纪念版（`3302:c200`）的固件只暴露 `usb.equalizer` 一项功能：8 段参量均衡 +
7 个出厂预设。其它型号还提供数字滤波、麦克风增益、声道平衡、增益档位、工作模式等，
协议实现已按设备配置表分别支持。

## 支持的设备

型号与能力表直接取自 App 内置的 `assets/device-config/*.json`，逐字节嵌入本工具：

| USB ID | 型号 | 协议 | 功能 |
|---|---|---|---|
| `3302:C200` | 10th Anniversary Edition | `ttgk_v1` | EQ |
| `3302:1291` | StringSnow | `ttgk_v1` | EQ、特殊/游戏/睡眠模式 |
| `3302:129E` | Tears | `ttgk_v1` | EQ、特殊模式 |
| `3302:39C3` | NICEHCK Octave | `ttgk_v1` | EQ、数字滤波、麦克风增益、声道平衡、OTA |
| `3302:4322` | NICEHCK PureAural | `ttgk_v1` | EQ、滤波、增益档位、工作模式、麦克风增益、声道平衡、音量 |
| `3302:1595` | NK1 | `ttgk_info_v1` | 只读信息 |
| `3302:332D` | NK1 Ultra | `ttgk_info_v1` | 只读信息 |
| `3302:3347` | NK1 Max | `ttgk_info_v1` | 只读信息 |
| `0666:0888` | USB20（OTA 引导） | `nk_ota_v1` | 固件升级 |
| 蓝牙 | YUANDAO OriG in | `origin_v1` | 无线型号，不在本工具范围 |

## 安装

### 方式一：Nix flake

```bash
nix run .#gui     # 启动图形界面
nix run .#cli     # 命令行
nix build         # 构建软件包
nix develop       # 开发环境
```

### 方式二：直接 cargo

```bash
cargo build --release
./target/release/nicehck-gui    # 图形界面
./target/release/nicehck list   # 命令行
```

需要 Rust 1.75+。GUI 依赖 `libGL`、`libxkbcommon`、wayland 或 X11 运行库。

### 方式三：NixOS 声明式

在 `flake.nix` 里加入本仓库作为 input，然后用自带的 NixOS 模块：

```nix
{
  inputs.nicehck-linux.url = "github:Mooling0602/nicehck-linux";

  outputs = { self, nixpkgs, nicehck-linux, ... }: {
    nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
      modules = [
        nicehck-linux.nixosModules.default
        { programs.nicehck.enable = true; }
      ];
    };
  };
}
```

**udev 规则随主包分发**，所以不需要第二个包、也没有"传错了静默失效"的坑：
主包里带着 `etc/udev/rules.d/70-nicehck.rules`，正是 `services.udev.packages`
扫描的位置。

```nix
{
  # 规则和程序来自同一个包
  services.udev.packages = [ inputs.nicehck-linux.packages.${pkgs.system}.default ];
  environment.systemPackages = [ inputs.nicehck-linux.packages.${pkgs.system}.default ];
}
```

两个开关也可以分开用（例如只授权、不把程序装进 PATH）：

```nix
programs.nicehck.udevRules = true;    # 只应用权限规则
programs.nicehck.install   = false;   # 不装 nicehck / nicehck-gui
```

> `packages.udevRules` 作为别名保留，指向同一个 store 路径。

## 权限

`/dev/hidraw*` 默认是 `root:root 0600`，非 root 用户直接打不开。

### NixOS 用户

用上面的 `programs.nicehck.udevRules = true`，或用裸 Nix：

```nix
services.udev.packages = [ inputs.nicehck-linux.packages.${pkgs.system}.default ];
```

**不要**按下面的手动方式 `cp`：NixOS 的 `/etc/udev/rules.d` 是指向
`/nix/store/…-udev-rules` 的**只读软链**，写入会直接报错。

### 其它发行版

```bash
sudo cp udev/70-nicehck.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

### 临时试一下（不改任何配置）

```bash
sudo setfacl -m u:$USER:rw /dev/hidraw3
```

立即生效，但拔插或重启后失效，需要重跑。

### 原理与注意事项

规则用 `TAG+="uaccess"`：systemd-logind 会给**占用当前 seat 的登录用户**加 ACL，
因此**不需要**加组、也不需要重新登录，插拔一次耳机即可。

- 规则文件名必须是 `70-` 前缀。真正执行 uaccess 的是 `73-seat-late.rules`，
  udev 按文件名字典序处理，排在它后面就来不及了。经实测：
  `70-nicehck.rules` 排在第 380 行、`73-seat-late.rules` 在第 385 行；
  而 `services.udev.extraRules` 会写成 `99-local.rules`（第 436 行），
  **对这种"先打标签、后消费标签"的规则无效**。
- 文件里那条 `GROUP="users"` 是**注释掉的**。udev 无法表达"仅在缺少 logind 时生效"，
  一旦启用就会在所有系统上生效；而 NixOS 的 `users` 组（gid 100）包含所有普通账户，
  权限面比 uaccess 宽。真有需要请换成专用组。

- 规则还顺手关掉了该设备的 USB autosuspend。挂起帧会让 HID 端点丢回复，
  表现为命令超时，看起来和"设备坏了"一模一样。

NixOS 用户建议走上面的声明式方式，规则留在 store 里不会漂移。

## 命令行用法

```bash
nicehck list                  # 列出已连接的耳机与已知型号
nicehck info                  # 设备身份、能力范围、出厂预设
nicehck eq                    # 读取当前 EQ（安全，不改声音）
nicehck eq --hexdump          # 附带原始报告字节，便于核对协议
nicehck eq --json             # 机器可读输出
nicehck status                # 固件版本、音量、滤波、增益、工作模式、平衡
nicehck --device /dev/hidraw3 eq    # 指定设备节点
```

`--write` 预留用于写入路径；当前版本的写入功能仍在逐项对真机验证。

## 图形界面

`nicehck-gui` 提供：

- **Equalizer 页** — 实时频响曲线（按 DSP 实际传输函数计算，非示意插值）、
  8 段增益/Q 滑条、offset 前级、出厂预设一览
- **Device 页** — USB 身份、协议变体、能力范围、预设列表、**写入解锁开关**与操作日志
- **About 页** — 协议要点与权限说明

界面默认**只读**。任何会改变声音的操作都需要先在 Device 页手动解锁写入；
解锁状态在底部状态栏持续可见。

## 安全设计

耳机是正在出声的音频设备，写错参数你**立刻**能听到。因此：

1. 默认只读，写入需显式解锁
2. 所有会改声音的操作都在 UI 上有明确标识
3. 读取路径走独立线程，设备无响应不会卡死界面
4. 权限错误直接给出可执行的修复命令，不静默失败

## 项目结构

```
crates/nicehck-protocol/    协议库（无 GUI 依赖，可单独复用）
  src/crc.rs                CRC-32
  src/biquad.rs             peaking biquad 系数与 Q30 量化
  src/command.rs            报告编解码 + 全部命令
  src/frame.rs              OTA 帧格式（BUXX 魔数）
  src/device_config.rs      App 内置设备配置表
  src/transport.rs          /dev/hidraw I/O
  data/device-config/       从 APK 提取的设备配置原始 JSON（经 include_str! 编译进二进制）
crates/nicehck-cli/         命令行
crates/nicehck-gui/         图形界面（egui）
  src/app.rs                界面与状态机
  src/curve.rs              频响曲线计算
udev/70-nicehck.rules       udev 规则
docs/PROTOCOL.md            协议逆向记录
flake.nix                   Nix 打包；udev 规则随主包分发，另有 nixosModules.default
```

产物不使用外部数据文件：设备配置表在编译期嵌入，运行时不读磁盘。

## 测试

```bash
cargo test --workspace
```

45 个测试覆盖 CRC 向量、帧编解码与重同步、biquad 系数、Q30 量化、
命令载荷逐字节布局、设备配置解析、以及频响曲线数学。

## 许可

MIT
