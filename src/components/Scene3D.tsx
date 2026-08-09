import { useRef, useMemo } from 'react';
import { Canvas, useFrame } from '@react-three/fiber';
import { Float, MeshDistortMaterial } from '@react-three/drei';
import * as THREE from 'three';
import { useStore } from '../stores/appStore';
import type { AvatarState } from '../types';

function AvatarCore() {
  const avatarState = useStore((s) => s.avatarState);
  const ref = useRef<THREE.Mesh>(null);
  const ringRef = useRef<THREE.Mesh>(null);

  useFrame((_, delta) => {
    if (!ref.current) return;
    const speed = avatarState === 'thinking' ? 2.5 : avatarState === 'speaking' ? 1.8 : 0.8;
    ref.current.rotation.y += delta * speed * 0.3;
    ref.current.rotation.x = Math.sin(Date.now() * 0.001 * speed) * 0.1;

    const scale = avatarState === 'speaking'
      ? 1 + Math.sin(Date.now() * 0.008) * 0.08
      : avatarState === 'thinking'
        ? 1 + Math.sin(Date.now() * 0.005) * 0.05
        : avatarState === 'listening'
          ? 1.05
          : 1;
    ref.current.scale.setScalar(scale);

    if (ringRef.current) {
      ringRef.current.rotation.z += delta * (avatarState === 'idle' ? 0.2 : 0.6);
    }
  });

  const color = avatarState === 'thinking' ? '#ff6b9d' : avatarState === 'speaking' ? '#64b5f6' : '#9b59f5';
  const emissive = avatarState === 'listening' ? 1.2 : 0.7;

  return (
    <group position={[0, 0.5, -2]}>
      <Float speed={avatarState === 'idle' ? 1.5 : 3} rotationIntensity={0.3} floatIntensity={1.2}>
        <mesh ref={ref}>
          <icosahedronGeometry args={[1.2, 5]} />
          <MeshDistortMaterial
            color={color}
            emissive={color}
            emissiveIntensity={emissive}
            transparent
            opacity={0.55}
            distort={avatarState === 'thinking' ? 0.6 : 0.35}
            speed={avatarState === 'thinking' ? 4 : 2}
            roughness={0.05}
            metalness={0.9}
          />
        </mesh>
      </Float>
      <mesh ref={ringRef} rotation={[Math.PI / 2, 0, 0]}>
        <torusGeometry args={[2, 0.02, 8, 64]} />
        <meshBasicMaterial color="#a8d8ff" transparent opacity={0.25} />
      </mesh>
      <mesh rotation={[Math.PI / 2, 0, 0]}>
        <torusGeometry args={[2.5, 0.015, 8, 64]} />
        <meshBasicMaterial color="#9b59f5" transparent opacity={0.15} />
      </mesh>
    </group>
  );
}

function GlowOrb({ position, color, scale = 1, state }: {
  position: [number, number, number]; color: string; scale?: number; state: AvatarState;
}) {
  const ref = useRef<THREE.Mesh>(null);
  useFrame((_, delta) => {
    if (ref.current) ref.current.rotation.y += delta * (state === 'thinking' ? 0.4 : 0.15);
  });

  return (
    <Float speed={2} rotationIntensity={0.4} floatIntensity={1.5}>
      <mesh ref={ref} position={position} scale={scale}>
        <icosahedronGeometry args={[1, 4]} />
        <MeshDistortMaterial
          color={color}
          emissive={color}
          emissiveIntensity={0.5}
          transparent
          opacity={0.3}
          distort={0.4}
          speed={2}
          roughness={0.1}
          metalness={0.8}
        />
      </mesh>
    </Float>
  );
}

function GlassPanel({ position, rotation }: { position: [number, number, number]; rotation?: [number, number, number] }) {
  const ref = useRef<THREE.Mesh>(null);
  useFrame((_, delta) => {
    if (ref.current) ref.current.rotation.z += delta * 0.05;
  });

  return (
    <Float speed={1.5} rotationIntensity={0.2} floatIntensity={0.8}>
      <mesh ref={ref} position={position} rotation={rotation ?? [0, 0, 0]}>
        <boxGeometry args={[3, 2, 0.05]} />
        <meshPhysicalMaterial
          color="#a8d8ff"
          transparent
          opacity={0.12}
          roughness={0}
          metalness={0.1}
          transmission={0.95}
          thickness={0.5}
          ior={1.5}
        />
      </mesh>
    </Float>
  );
}

function ParticleField() {
  const count = 250;
  const positions = useMemo(() => {
    const pos = new Float32Array(count * 3);
    for (let i = 0; i < count; i++) {
      pos[i * 3] = (Math.random() - 0.5) * 30;
      pos[i * 3 + 1] = (Math.random() - 0.5) * 20;
      pos[i * 3 + 2] = (Math.random() - 0.5) * 15 - 5;
    }
    return pos;
  }, []);

  const ref = useRef<THREE.Points>(null);
  useFrame((_, delta) => {
    if (ref.current) ref.current.rotation.y += delta * 0.02;
  });

  return (
    <points ref={ref}>
      <bufferGeometry>
        <bufferAttribute attach="attributes-position" count={count} array={positions} itemSize={3} />
      </bufferGeometry>
      <pointsMaterial size={0.04} color="#9b59f5" transparent opacity={0.6} sizeAttenuation />
    </points>
  );
}

export function Scene3D() {
  const avatarState = useStore((s) => s.avatarState);

  return (
    <Canvas
      camera={{ position: [0, 0, 8], fov: 60 }}
      style={{ position: 'absolute', inset: 0, zIndex: 0 }}
      gl={{ antialias: true, alpha: true }}
    >
      <color attach="background" args={['#050510']} />
      <ambientLight intensity={0.2} />
      <pointLight position={[5, 5, 5]} intensity={1.2} color="#9b59f5" />
      <pointLight position={[-5, -3, 3]} intensity={0.9} color="#64b5f6" />
      <spotLight position={[0, 5, 2]} intensity={0.8} color="#b388ff" angle={0.5} />

      <AvatarCore />

      <GlowOrb position={[-5, 2, -4]} color="#9b59f5" scale={0.9} state={avatarState} />
      <GlowOrb position={[5, -1, -5]} color="#7c4dff" scale={0.7} state={avatarState} />
      <GlowOrb position={[-3, -2, -3]} color="#b388ff" scale={0.5} state={avatarState} />

      <GlassPanel position={[4, 2, -3]} rotation={[0.2, -0.3, 0.1]} />
      <GlassPanel position={[-5, -1, -2]} rotation={[-0.1, 0.4, -0.15]} />
      <GlassPanel position={[2, -3, -4]} rotation={[0.3, 0.1, 0.2]} />

      <ParticleField />
    </Canvas>
  );
}
